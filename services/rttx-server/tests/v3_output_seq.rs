//! End-to-end tests for v3 `pane_output_seq` continuity.
//!
//! Verifies that the live daemon path produces correct per-pane output
//! sequences in both attach snapshots and pushed `OutputDelta` messages,
//! as required by RFC-021.

mod common;

use rttx_proto::v3;

/// Attach snapshot includes the current `pane_output_seq` for each pane.
#[tokio::test]
async fn snapshot_carries_pane_output_seq() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, _handle) = common::start_test_server(tmp.path()).await;

    let mut client = common::TestV3Client::connect(&socket_path).await;
    client.handshake().await;
    let runtime_id =
        common::create_workspace(&mut client, "seq-snap", v3::WorkspacePolicy::Persistent).await;
    let _snap = common::attach_rw(&mut client, &runtime_id).await;
    let pane_id = common::create_pane(&mut client, &runtime_id).await;

    // Drain initial shell output so the pane's output_seq advances.
    let _ = client.collect_output_seqs(std::time::Duration::from_secs(2)).await;

    // Detach and reattach — snapshot should carry the current seq.
    let env = rttx_proto::v3::ClientEnvelope {
        request_id: 100,
        command: Some(rttx_proto::v3::client_envelope::Command::DetachWorkspace(
            rttx_proto::v3::DetachWorkspace { runtime_id: runtime_id.clone() },
        )),
    };
    client.send(&env).await;
    // Drain until detached.
    loop {
        let resp = client.recv().await;
        if matches!(
            resp.payload,
            Some(
                rttx_proto::v3::server_envelope::Payload::WorkspaceDetached(_)
                    | rttx_proto::v3::server_envelope::Payload::WorkspaceTerminated(_)
            )
        ) {
            break;
        }
    }

    let snap = common::attach_rw(&mut client, &runtime_id).await;
    assert!(!snap.panes.is_empty(), "snapshot should contain the pane");
    let pane_snap = snap.panes.iter().find(|p| p.pane_id == pane_id).expect("pane not in snapshot");
    // The pane had output, so seq must be > 0.
    assert!(
        pane_snap.pane_output_seq > 0,
        "snapshot pane_output_seq should reflect prior output, got {}",
        pane_snap.pane_output_seq
    );
}

/// Live `OutputDelta` messages carry contiguous `pane_output_seq` starting
/// from `snapshot_seq + 1`.
#[tokio::test]
async fn live_deltas_carry_contiguous_pane_output_seq() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, _handle) = common::start_test_server(tmp.path()).await;

    let mut client = common::TestV3Client::connect(&socket_path).await;
    client.handshake().await;
    let runtime_id =
        common::create_workspace(&mut client, "seq-delta", v3::WorkspacePolicy::Persistent).await;
    let _snap = common::attach_rw(&mut client, &runtime_id).await;
    let _pane_id = common::create_pane(&mut client, &runtime_id).await;

    // Collect output deltas from shell startup.
    let seqs = client.collect_output_seqs(std::time::Duration::from_secs(2)).await;

    // Must have received at least one delta.
    assert!(!seqs.is_empty(), "expected at least one OutputDelta from shell startup");

    // All seqs must be > 0 (not the hardcoded 0).
    for (i, &seq) in seqs.iter().enumerate() {
        assert!(seq > 0, "delta[{i}] pane_output_seq must be > 0, got {seq}");
    }

    // Seqs must be contiguous (each increments by 1).
    for window in seqs.windows(2) {
        assert_eq!(
            window[1],
            window[0] + 1,
            "pane_output_seq must be contiguous: {} followed by {}",
            window[0],
            window[1]
        );
    }
}

/// After attach, the next delta carries `snapshot_seq + 1`.
#[tokio::test]
async fn delta_seq_continues_from_snapshot_seq() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, _handle) = common::start_test_server(tmp.path()).await;

    let mut client = common::TestV3Client::connect(&socket_path).await;
    client.handshake().await;
    let runtime_id =
        common::create_workspace(&mut client, "seq-cont", v3::WorkspacePolicy::Persistent).await;
    let _snap = common::attach_rw(&mut client, &runtime_id).await;
    let pane_id = common::create_pane(&mut client, &runtime_id).await;

    // Drain initial output.
    let _ = client.collect_output_seqs(std::time::Duration::from_secs(2)).await;

    // Detach.
    let env = rttx_proto::v3::ClientEnvelope {
        request_id: 200,
        command: Some(rttx_proto::v3::client_envelope::Command::DetachWorkspace(
            rttx_proto::v3::DetachWorkspace { runtime_id: runtime_id.clone() },
        )),
    };
    client.send(&env).await;
    loop {
        let resp = client.recv().await;
        if matches!(
            resp.payload,
            Some(
                rttx_proto::v3::server_envelope::Payload::WorkspaceDetached(_)
                    | rttx_proto::v3::server_envelope::Payload::WorkspaceTerminated(_)
            )
        ) {
            break;
        }
    }

    // Reattach and record snapshot seq.
    let snap = common::attach_rw(&mut client, &runtime_id).await;
    let pane_snap = snap.panes.iter().find(|p| p.pane_id == pane_id).expect("pane not in snapshot");
    let snapshot_seq = pane_snap.pane_output_seq;

    // Send input to trigger output.
    common::send_input(&mut client, &runtime_id, &pane_id, b"echo hello\n").await;

    // Collect deltas.
    let seqs = client.collect_output_seqs(std::time::Duration::from_secs(3)).await;
    assert!(!seqs.is_empty(), "expected output after echo");

    // First delta must be snapshot_seq + 1.
    assert_eq!(
        seqs[0],
        snapshot_seq + 1,
        "first delta after reattach should be snapshot_seq({snapshot_seq}) + 1, got {}",
        seqs[0]
    );
}

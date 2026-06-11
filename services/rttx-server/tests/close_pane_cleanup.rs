//! Close-driven pane cleanup (RFC-031 §8, issue #1014).
//!
//! The orphan-sweep that masked the history-loss bug is gone. Its replacement
//! is explicit, close-driven cleanup keyed on pane-tree membership: when a pane
//! leaves the tree via `ClosePane`, its durable artifacts (history, scrollback,
//! screen snapshot) are removed. Panes that remain in the tree keep theirs.

mod common;

use common::*;
use rttx_proto::v3;
use std::path::Path;
use std::time::Duration;

async fn poll_until(deadline: tokio::time::Instant, mut cond: impl FnMut() -> bool) -> bool {
    while tokio::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    cond()
}

/// All three durable artifacts for a pane exist on disk.
fn pane_artifacts_present(state_dir: &Path, runtime_id: uuid::Uuid, pane_id: uuid::Uuid) -> bool {
    rttx_server::state::layout::history_file(state_dir, runtime_id, pane_id).exists()
        && rttx_server::state::layout::screen_snapshot(state_dir, runtime_id, pane_id).exists()
        && rttx_server::state::layout::scrollback_log(state_dir, runtime_id, pane_id).exists()
}

/// No durable artifact for a pane remains on disk.
fn pane_artifacts_absent(state_dir: &Path, runtime_id: uuid::Uuid, pane_id: uuid::Uuid) -> bool {
    !rttx_server::state::layout::history_file(state_dir, runtime_id, pane_id).exists()
        && !rttx_server::state::layout::screen_snapshot(state_dir, runtime_id, pane_id).exists()
        && !rttx_server::state::layout::scrollback_log(state_dir, runtime_id, pane_id).exists()
}

/// Closing one pane of a multi-pane workspace removes exactly that pane's
/// durable state and leaves the surviving pane's state intact.
#[tokio::test]
async fn closing_a_pane_removes_only_its_durable_state() {
    let tmp = tempfile::TempDir::new().unwrap();
    let state_dir = tmp.path().join("state/rttx/daemon");

    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let runtime_id =
        create_workspace(&mut client, "close-cleanup", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client, &runtime_id).await;

    let keep = create_pane(&mut client, &runtime_id).await;
    let close = split_pane(&mut client, &runtime_id, &keep, v3::PaneSplitAxis::Horizontal, 0.5)
        .await
        .new_pane_id;

    // Drive a marker into each pane's per-pane HISTFILE so history lands on disk
    // independent of the host's $SHELL.
    for pane in [&keep, &close] {
        send_input(&mut client, &runtime_id, pane, b"PROMPT_COMMAND='history -a'\n").await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        send_input(&mut client, &runtime_id, pane, b"echo close_cleanup_marker\n").await;
        send_input(&mut client, &runtime_id, pane, b"true\n").await;
    }

    let workspace_uuid = rttx_proto::bytes_to_uuid(&runtime_id).unwrap();
    let keep_uuid = rttx_proto::bytes_to_uuid(&keep).unwrap();
    let close_uuid = rttx_proto::bytes_to_uuid(&close).unwrap();

    // Wait for both panes' durable artifacts to reach disk before closing.
    wait_for_state_containing(tmp.path(), "close-cleanup", Duration::from_secs(10)).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let both_durable = poll_until(deadline, || {
        pane_artifacts_present(&state_dir, workspace_uuid, keep_uuid)
            && pane_artifacts_present(&state_dir, workspace_uuid, close_uuid)
    })
    .await;
    assert!(both_durable, "both panes' artifacts must reach disk before close");

    // Close one pane: its durable state must be swept by the close-driven cleanup.
    close_pane(&mut client, &runtime_id, &close).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let closed_cleaned =
        poll_until(deadline, || pane_artifacts_absent(&state_dir, workspace_uuid, close_uuid))
            .await;
    assert!(
        closed_cleaned,
        "closed pane's history/scrollback/screen must be removed by close-driven cleanup"
    );

    // The surviving pane keeps every durable artifact.
    assert!(
        pane_artifacts_present(&state_dir, workspace_uuid, keep_uuid),
        "surviving pane's durable state must be untouched by closing a sibling"
    );
}

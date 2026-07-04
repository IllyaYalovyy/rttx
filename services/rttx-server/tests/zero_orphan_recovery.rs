//! Zero-orphan crash-recovery integration test (RFC-031 §8, Step 6, issue #1003).
//!
//! Durable pane state must key on stable server pane ids, not randomly-minted,
//! process-ephemeral pane ids. A reconnect or a respawn could change the id and
//! silently orphan the pane's history/scrollback/screen; a startup orphan-sweep
//! then *hid* that data loss by quietly relocating the unreferenced directories.
//!
//! With a server-authoritative, immutable `PaneId`, every durable artifact is
//! keyed on an id that never changes across shell respawn, daemon crash, or
//! restart. This test proves the invariant end-to-end across *repeated* hard
//! crashes:
//!
//! 1. every pane keeps its `PaneId` (the reattach snapshot tree is identical);
//! 2. each pane's history, scrollback, and screen snapshot survive on disk;
//! 3. **no orphaned state is ever produced** — exactly one workspace directory
//!    exists and the `.orphans/` sweep target is never populated, because
//!    nothing is ever left unreferenced.

mod common;

use common::*;
use rttx_proto::v3;
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

/// Collect every leaf pane id in a proto tree, left to right.
fn leaf_ids(node: &v3::PaneTreeNode) -> Vec<Vec<u8>> {
    fn walk(node: &v3::PaneTreeNode, out: &mut Vec<Vec<u8>>) {
        match node.node.as_ref() {
            Some(v3::pane_tree_node::Node::Leaf(leaf)) => out.push(leaf.pane_id.clone()),
            Some(v3::pane_tree_node::Node::Split(split)) => {
                if let Some(first) = split.first.as_ref() {
                    walk(first, out);
                }
                if let Some(second) = split.second.as_ref() {
                    walk(second, out);
                }
            }
            None => {}
        }
    }
    let mut out = Vec::new();
    walk(node, &mut out);
    out
}

/// Names of the workspace directories under `workspaces/`, excluding the
/// `.orphans/` sweep target, which is reported separately.
fn runtime_dir_names(state_dir: &Path) -> Vec<String> {
    let workspaces = state_dir.join("workspaces");
    let mut names = Vec::new();
    let Ok(entries) = std::fs::read_dir(&workspaces) else { return names };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name != ".orphans" {
            names.push(name);
        }
    }
    names
}

/// Number of entries currently parked in the `.orphans/` sweep target. Zero
/// proves nothing was ever left unreferenced for the sweep to relocate.
fn orphan_count(state_dir: &Path) -> usize {
    let orphans = state_dir.join("workspaces/.orphans");
    std::fs::read_dir(&orphans).map_or(0, |d| d.flatten().count())
}

async fn poll_until(deadline: tokio::time::Instant, mut cond: impl FnMut() -> bool) -> bool {
    while tokio::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    cond()
}

/// Create N panes (1 + splits), run a unique marker command in each, and return
/// the workspace id plus the per-pane (`pane_id`, marker) pairs.
async fn build_workspace_with_markers(
    client: &mut TestClient,
    pane_count: usize,
) -> (Vec<u8>, Vec<(Vec<u8>, String)>) {
    let runtime_id = create_workspace(client, "zero-orphan", v3::WorkspacePolicy::Persistent).await;
    attach_rw(client, &runtime_id).await;

    let first = create_pane(client, &runtime_id).await;
    let mut panes = vec![first.clone()];
    let mut target = first;
    for _ in 1..pane_count {
        let split =
            split_pane(client, &runtime_id, &target, v3::PaneSplitAxis::Horizontal, 0.5).await;
        panes.push(split.new_pane_id.clone());
        // Split the freshly minted pane next so the tree grows into a chain of
        // distinct leaves rather than collapsing onto one target.
        target = split.new_pane_id;
    }

    let mut markers = Vec::with_capacity(panes.len());
    for (idx, pane) in panes.iter().enumerate() {
        let marker = format!("ZERO_ORPHAN_MARKER_{idx}");
        // Enable incremental history flush so the marker reaches the per-pane
        // HISTFILE during normal operation, independent of the host's $SHELL.
        send_input(client, &runtime_id, pane, b"PROMPT_COMMAND='history -a'\n").await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        send_input(client, &runtime_id, pane, format!("echo {marker}\n").as_bytes()).await;
        send_input(client, &runtime_id, pane, b"true\n").await;
        markers.push((pane.clone(), marker));
    }

    (runtime_id, markers)
}

/// Daemon-level invariant: across repeated hard crashes a multi-pane persistent
/// workspace keeps every pane's identity and durable state, and never leaks an
/// orphaned directory.
#[tokio::test]
async fn multi_pane_workspace_survives_repeated_hard_crashes_without_orphans() {
    const PANE_COUNT: usize = 3;
    let tmp = tempfile::TempDir::new().unwrap();
    let state_dir = tmp.path().join("state/rttx/daemon");

    let runtime_id;
    let markers;
    let original_ids: BTreeSet<Vec<u8>>;

    // Phase 1: build the workspace and let durable state reach disk.
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        (runtime_id, markers) = build_workspace_with_markers(&mut client, PANE_COUNT).await;
        original_ids = markers.iter().map(|(id, _)| id.clone()).collect();

        let workspace_uuid = rttx_proto::bytes_to_uuid(&runtime_id).unwrap();

        // Wait for the metadata + per-pane durable artifacts to land. The
        // workspace is dirty after creation/splits/input, so the next
        // persistence tick writes workspace.json, screen snapshots, and
        // scrollback for every pane.
        wait_for_state_containing(tmp.path(), "zero-orphan", Duration::from_secs(10)).await;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let all_durable = poll_until(deadline, || {
            markers.iter().all(|(pane, marker)| {
                let pane_uuid = rttx_proto::bytes_to_uuid(pane).unwrap();
                let hist =
                    rttx_server::state::layout::history_file(&state_dir, workspace_uuid, pane_uuid);
                let screen = rttx_server::state::layout::screen_snapshot(
                    &state_dir,
                    workspace_uuid,
                    pane_uuid,
                );
                let scroll = rttx_server::state::layout::scrollback_log(
                    &state_dir,
                    workspace_uuid,
                    pane_uuid,
                );
                std::fs::read_to_string(&hist).unwrap_or_default().contains(marker)
                    && screen.exists()
                    && scroll.exists()
            })
        })
        .await;
        assert!(all_durable, "history/scrollback/screen must reach disk before the first crash");

        // Simulate a hard crash: abort with no clean shutdown.
        handle.abort();
        tokio::time::sleep(Duration::from_millis(150)).await;
    }

    let workspace_uuid = rttx_proto::bytes_to_uuid(&runtime_id).unwrap();

    // Phase 2: repeated crash/restart cycles. Each cycle must restore identity
    // and durable state and must never create an orphan.
    for cycle in 0..3 {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        // (c) No orphaned state: exactly one workspace dir, empty `.orphans/`.
        let dirs = runtime_dir_names(&state_dir);
        assert_eq!(
            dirs.len(),
            1,
            "cycle {cycle}: exactly one workspace directory must exist, found {dirs:?}"
        );
        assert_eq!(
            dirs[0],
            workspace_uuid.to_string(),
            "cycle {cycle}: workspace dir id is stable"
        );
        assert_eq!(
            orphan_count(&state_dir),
            0,
            "cycle {cycle}: nothing must ever be swept into .orphans/"
        );

        // (a) Identity: the reattach snapshot tree carries the same pane ids.
        let snapshot = attach_rw(&mut client, &runtime_id).await;
        let tree = snapshot.tree.as_ref().expect("snapshot must carry the authoritative tree");
        let restored_ids: BTreeSet<Vec<u8>> = leaf_ids(tree).into_iter().collect();
        assert_eq!(
            restored_ids, original_ids,
            "cycle {cycle}: every pane must keep its immutable PaneId across restart"
        );

        // (b) Durable state intact: history marker, scrollback, and screen
        // snapshot all survive for every pane, keyed on the stable id.
        for (pane, marker) in &markers {
            let pane_uuid = rttx_proto::bytes_to_uuid(pane).unwrap();
            let hist =
                rttx_server::state::layout::history_file(&state_dir, workspace_uuid, pane_uuid);
            let screen =
                rttx_server::state::layout::screen_snapshot(&state_dir, workspace_uuid, pane_uuid);
            let scroll =
                rttx_server::state::layout::scrollback_log(&state_dir, workspace_uuid, pane_uuid);
            assert!(
                std::fs::read_to_string(&hist).unwrap_or_default().contains(marker),
                "cycle {cycle}: history for pane must survive, expected {marker} in {}",
                hist.display()
            );
            assert!(
                screen.exists(),
                "cycle {cycle}: screen snapshot must survive at {}",
                screen.display()
            );
            assert!(
                scroll.exists(),
                "cycle {cycle}: scrollback log must survive at {}",
                scroll.display()
            );
        }

        // Let the reconstructed workspace re-persist (it is dirty after reattach)
        // so the next cycle observes a fully written state dir, then crash.
        wait_for_state_containing(tmp.path(), "zero-orphan", Duration::from_secs(10)).await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

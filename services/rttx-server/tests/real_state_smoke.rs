//! Smoke test against a *real* daemon state directory.
//!
//! Set `RTTX_SMOKE_STATE_ROOT` to a directory that contains
//! `state/rttx/daemon` copied from a machine (it is modified: the daemon
//! respawns shells and rewrites files). The test starts the daemon on that
//! state, attaches to every persistent workspace, and checks that every
//! pane's restored picture ends on its prompt with the cursor on that line —
//! never in the middle of the history. Skipped when the variable is unset.

mod common;

use common::*;
use std::time::Duration;

fn pane_id_matches(id: &[u8], prefix: &str) -> bool {
    uuid::Uuid::from_slice(id).is_ok_and(|u| u.to_string().starts_with(prefix))
}

#[tokio::test]
async fn every_restored_pane_has_its_cursor_on_the_last_line() {
    let Ok(root) = std::env::var("RTTX_SMOKE_STATE_ROOT") else {
        eprintln!("RTTX_SMOKE_STATE_ROOT not set; skipping");
        return;
    };
    let root = std::path::PathBuf::from(root);
    let _ =
        tracing_subscriber::fmt().with_env_filter("info").with_writer(std::io::stderr).try_init();
    let started = std::time::Instant::now();
    assert!(root.join("state/rttx/daemon").is_dir(), "no daemon state under {}", root.display());

    let (sock, _handle) = start_test_server(&root).await;
    eprintln!("daemon up after {:?}", started.elapsed());
    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    // Let the respawned shells print their prompts.
    tokio::time::sleep(Duration::from_secs(3)).await;

    let workspaces = list_workspaces(&mut client).await;
    assert!(!workspaces.is_empty(), "the state directory should hold workspaces");
    let mut checked = 0;
    let mut report = Vec::new();
    for ws in &workspaces {
        let snapshot = attach_rw(&mut client, &ws.id).await;
        for pane in &snapshot.panes {
            if pane.exit_status.is_some() || pane.scrollback_tail.is_empty() {
                continue;
            }
            let (cols, rows) = (pane.cols.max(1) as u16, pane.rows.max(1) as u16);
            let mut view = vt100::Parser::new(rows, cols, 5000);
            view.process(&pane.scrollback_tail);
            let lines: Vec<String> =
                view.screen().rows(0, cols).map(|r| r.trim_end().to_string()).collect();
            let (cursor_row, _) = view.screen().cursor_position();
            let last_content = lines.iter().rposition(|l| !l.is_empty()).unwrap_or(0);
            let ok = usize::from(cursor_row) == last_content;
            report.push(format!(
                "{} {:<20} pane {} {}x{} cursor_row={} last_content_row={} last={:?} {}",
                if ok { "OK  " } else { "BAD " },
                ws.name,
                &uuid::Uuid::from_slice(&pane.pane_id).unwrap().to_string()[..8],
                cols,
                rows,
                cursor_row,
                last_content,
                lines[last_content].chars().take(60).collect::<String>(),
                if view.screen().alternate_screen() { "(alt screen)" } else { "" },
            ));
            checked += 1;
            if std::env::var("RTTX_SMOKE_DUMP").is_ok_and(|v| pane_id_matches(&pane.pane_id, &v)) {
                for (i, l) in lines.iter().enumerate() {
                    eprintln!("  row {i:>2}: {l:?}");
                }
            }
        }
        detach_workspace(&mut client, &ws.id).await;
    }
    for line in &report {
        eprintln!("{line}");
    }
    assert!(checked > 0, "no live panes to check");
    let bad: Vec<&String> = report.iter().filter(|l| l.starts_with("BAD")).collect();
    assert!(
        bad.is_empty(),
        "{} pane(s) restored with the cursor off the last line:\n{}",
        bad.len(),
        bad.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n")
    );
}

/// Replay one persisted screen snapshot file (`RTTX_SMOKE_SNAP`) through
/// the restart path and print the grid, for diagnosing a single pane.
#[test]
fn replay_one_snapshot_file() {
    let Ok(path) = std::env::var("RTTX_SMOKE_SNAP") else { return };
    let text = std::fs::read_to_string(&path).unwrap();
    let snap: rttx_server::state::types::ScreenSnapshotV1 = serde_json::from_str(&text).unwrap();
    let dump = |pane: &rttx_server::pane::Pane, label: &str| {
        let (r, c) = pane.screen.cursor_position();
        eprintln!("== {label}: cursor=({r},{c})");
    };
    let mut pane = rttx_server::pane::Pane::new(snap.pane_id, snap.cols, snap.rows);
    pane.restore_from_snapshot(&snap);
    dump(&pane, "after restore");
    let stream = pane.screen.reattach_stream();
    let mut view = vt100::Parser::new(snap.rows, snap.cols, 100);
    view.process(&stream);
    for (i, l) in view.screen().rows(0, snap.cols).enumerate() {
        let l = l.trim_end();
        if !l.is_empty() {
            eprintln!("  row {i:>2}: {l:?}");
        }
    }
    eprintln!("view cursor {:?}", view.screen().cursor_position());
    pane.feed_output(b"PROMPT> ");
    let stream = pane.screen.reattach_stream();
    let mut view = vt100::Parser::new(snap.rows, snap.cols, 100);
    view.process(&stream);
    for (i, l) in view.screen().rows(0, snap.cols).enumerate().skip(50) {
        eprintln!("  after prompt row {i:>2}: {:?}", l.trim_end());
    }
}

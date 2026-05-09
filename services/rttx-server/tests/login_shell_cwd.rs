//! Regression test: shells spawned by the daemon are login shells so that
//! `/etc/profile.d/vte-2.91.sh` is sourced and OSC 7 CWD reporting works.

mod common;

use common::{TestClient, send_input, start_test_server};
use rttx_proto::proto;
use std::time::Duration;

#[tokio::test]
async fn spawned_shell_is_login_shell_and_reports_cwd_via_osc7() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let runtime_id = common::create_runtime(&mut client, "login-shell-test", proto::RuntimePolicy::Persistent).await;

    let create_pane = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
            runtime_id: runtime_id.clone(),
            cwd: None,
            dark_background: None,
            cols: 0,
            rows: 0,
            no_persist: None,
        })),
    };
    client.send(&create_pane).await;
    let pane_id = match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    let attach = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
            runtime_id: runtime_id.clone(),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::Snapshot(_)) => {}
        other => panic!("expected Snapshot, got {other:?}"),
    }

    // cd to /tmp — if the shell is a login shell, PROMPT_COMMAND will emit
    // OSC 7 with the new CWD after the command completes.
    send_input(&mut client, &runtime_id, &pane_id, b"cd /tmp\n").await;

    let msgs = client.drain(Duration::from_secs(5)).await;
    let saw_tmp_cwd = msgs.iter().any(|m| matches!(
        &m.msg,
        Some(proto::server_message::Msg::CwdChanged(c)) if c.cwd == "/tmp"
    ));

    assert!(
        saw_tmp_cwd,
        "login shell should emit OSC 7 after cd, triggering CwdChanged with /tmp"
    );
}

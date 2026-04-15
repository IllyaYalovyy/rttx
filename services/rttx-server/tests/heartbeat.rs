//! Integration tests for daemon heartbeat messages.

mod common;

use common::{TestClient, attach_rw, create_session, start_test_server};
use rttx_proto::proto;

#[test]
fn ping_receives_matching_pong() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let tmp = tempfile::TempDir::new().unwrap();
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::Ping(proto::Ping { nonce: 42 })),
            })
            .await;

        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::Pong(pong)) => assert_eq!(pong.nonce, 42),
            other => panic!("expected Pong, got {other:?}"),
        }
    });
}

#[test]
fn ping_roundtrip_still_works_for_attached_clients() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let tmp = tempfile::TempDir::new().unwrap();
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        let session_id =
            create_session(&mut client, "heartbeat-attach", proto::RuntimePolicy::Persistent).await;
        let _snapshot = attach_rw(&mut client, &session_id).await;

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::Ping(proto::Ping { nonce: 7 })),
            })
            .await;

        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::Pong(pong)) => assert_eq!(pong.nonce, 7),
            other => panic!("expected Pong, got {other:?}"),
        }
    });
}

#[test]
fn ping_answered_while_mutex_held() {
    // Regression test for #556: the client_reader fast-path must respond
    // to Ping without acquiring the server mutex. If the mutex is held
    // (e.g. by a long-running PTY read loop), Pong must still arrive
    // promptly so the client heartbeat does not time out.
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let tmp = tempfile::TempDir::new().unwrap();
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        let session_id =
            create_session(&mut client, "mutex-held", proto::RuntimePolicy::Persistent).await;
        let _snapshot = attach_rw(&mut client, &session_id).await;

        // Create a pane and trigger continuous output to keep the mutex busy.
        let pane_id = common::create_pane(&mut client, &session_id).await;

        // Generate a burst of PTY output that keeps the server busy.
        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::Input(proto::Input {
                    session_id: session_id.clone(),
                    pane_id,
                    data: bytes::Bytes::from_static(
                        b"for i in $(seq 1 500); do echo line$i; done\n",
                    ),
                })),
            })
            .await;

        // Immediately send multiple pings — they must all be answered
        // promptly even while PTY output is being processed.
        for nonce in [100, 200, 300] {
            client
                .send(&proto::ClientMessage {
                    msg: Some(proto::client_message::Msg::Ping(proto::Ping { nonce })),
                })
                .await;
        }

        // Collect all three pongs within a tight deadline.
        let mut pong_nonces = Vec::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while pong_nonces.len() < 3 && tokio::time::Instant::now() < deadline {
            match client.try_recv(std::time::Duration::from_secs(2)).await {
                Some(msg) => {
                    if let Some(proto::server_message::Msg::Pong(pong)) = msg.msg {
                        pong_nonces.push(pong.nonce);
                    }
                }
                None => break,
            }
        }
        assert_eq!(pong_nonces, vec![100, 200, 300], "all pongs should arrive promptly");
    });
}

#[test]
fn ping_answered_during_pty_output() {
    // Regression test for #532: the server must answer pings even when
    // push messages (PTY deltas) are being sent concurrently. The old
    // single-loop design could deadlock when the write path blocked the
    // read path.
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let tmp = tempfile::TempDir::new().unwrap();
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        let session_id =
            create_session(&mut client, "ping-during-output", proto::RuntimePolicy::Persistent)
                .await;
        let _snapshot = attach_rw(&mut client, &session_id).await;

        // Create a pane that will produce output.
        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                    session_id: session_id.clone(),
                    cwd: None,
                    dark_background: Some(true),
                })),
            })
            .await;

        // Read PaneCreated response.
        let pane_id = loop {
            let msg = client.recv_or_timeout().await;
            if let Some(proto::server_message::Msg::PaneCreated(created)) = msg.msg {
                break created.pane_id;
            }
        };

        // Send some input to generate PTY output.
        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::Input(proto::Input {
                    session_id: session_id.clone(),
                    pane_id,
                    data: bytes::Bytes::from_static(b"echo hello\n"),
                })),
            })
            .await;

        // Wait briefly for output to start flowing.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Send a ping while output may be in flight.
        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::Ping(proto::Ping { nonce: 99 })),
            })
            .await;

        // Drain messages until we see the pong. With the concurrent
        // reader/writer design, the pong should arrive promptly even
        // if deltas are being sent.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut got_pong = false;
        while tokio::time::Instant::now() < deadline {
            match client.try_recv(std::time::Duration::from_secs(2)).await {
                Some(msg) => {
                    if let Some(proto::server_message::Msg::Pong(pong)) = msg.msg {
                        assert_eq!(pong.nonce, 99);
                        got_pong = true;
                        break;
                    }
                }
                None => break,
            }
        }
        assert!(got_pong, "pong should arrive even during PTY output");
    });
}

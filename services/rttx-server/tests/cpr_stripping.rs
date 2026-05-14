//! Regression test: CPR responses (ESC[row;colR) are stripped from output
//! sent to clients, preventing garbage text after daemon restart.

mod common;

use common::{TestClient, send_input, start_test_server};
use rttx_proto::proto;
use std::time::Duration;

#[tokio::test]
async fn cpr_responses_stripped_from_client_output() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let runtime_id =
        common::create_runtime(&mut client, "cpr-test", proto::RuntimePolicy::Persistent).await;
    let pane_id = common::create_pane(&mut client, &runtime_id).await;
    common::attach_rw(&mut client, &runtime_id).await;

    // Make the shell emit a CPR response by sending ESC[6n (cursor position query).
    // The shell's terminal driver responds with ESC[row;colR.
    // This response must NOT appear in the Delta messages sent to the client.
    send_input(&mut client, &runtime_id, &pane_id, b"printf '\\033[6n'; cat\n").await;

    let msgs = client.drain(Duration::from_secs(5)).await;

    // Check that no Delta contains a raw CPR response pattern.
    for msg in &msgs {
        if let Some(proto::server_message::Msg::Delta(delta)) = &msg.msg {
            let data = &delta.data;
            // CPR response: ESC [ <digits> ; <digits> R
            // Should have been stripped by strip_client_queries.
            let has_cpr = data.windows(3).any(|w| {
                w[0] == 0x1b
                    && w[1] == b'['
                    && data[w.as_ptr() as usize - data.as_ptr() as usize..]
                        .iter()
                        .skip(2)
                        .take_while(|&&b| b.is_ascii_digit() || b == b';')
                        .count()
                        > 0
                    && data
                        .get(
                            w.as_ptr() as usize - data.as_ptr() as usize
                                + 2
                                + data[w.as_ptr() as usize - data.as_ptr() as usize..]
                                    .iter()
                                    .skip(2)
                                    .take_while(|&&b| b.is_ascii_digit() || b == b';')
                                    .count(),
                        )
                        .copied()
                        == Some(b'R')
            });
            assert!(!has_cpr, "Delta should not contain CPR response (ESC[row;colR)");
        }
    }
}

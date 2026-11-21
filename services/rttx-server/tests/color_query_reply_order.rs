//! Integration test: replies to terminal queries reach the program in the
//! order the queries were asked, even though the daemon answers some itself
//! and the attached client answers others a round trip later.
//!
//! Programs such as `gh` ask for the background colour (OSC 11) and then for
//! the cursor position (DSR), and treat the cursor reply as "no more
//! answers". When the daemon's cursor reply overtook the client's colour
//! reply, the late colour reply landed in the program's input as garbage.

mod common;

use common::{TestClient, attach_rw, create_pane, create_workspace, start_test_server};
use rttx_proto::v3;
use std::time::Duration;

/// What the client's terminal widget would answer to `OSC 11 ; ?`.
const COLOR_REPLY: &[u8] = b"\x1b]11;rgb:1111/2222/3333\x1b\\";

/// Asks for the background colour, then the cursor position, reads up to the
/// `R` that ends the cursor reply and prints everything it read as hex. The
/// result markers are split with `''` so the echoed command line cannot be
/// mistaken for the result.
const PROBE: &str = r#"printf '\033]11;?\033\\\033[6n'; read -rs -d R REPLY; printf 'GO''T=%s=EN''D\n' "$(printf %s "$REPLY" | od -An -tx1 | tr -d ' \n')""#;

async fn send_raw(client: &mut TestClient, runtime_id: &[u8], pane_id: &[u8], data: &[u8]) {
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
                runtime_id: runtime_id.to_vec(),
                pane_id: pane_id.to_vec(),
                kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                    data: bytes::Bytes::copy_from_slice(data),
                })),
            })),
        })
        .await;
}

/// Collect pane output until `needle` shows up; returns everything seen.
async fn output_until(client: &mut TestClient, seen: &mut Vec<u8>, needle: &[u8]) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !seen.windows(needle.len()).any(|window| window == needle) {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let Some(msg) = client.try_recv(remaining).await else {
            panic!(
                "timed out waiting for {:?}; output so far: {:?}",
                String::from_utf8_lossy(needle),
                String::from_utf8_lossy(seen)
            );
        };
        if let Some(v3::server_envelope::Payload::OutputDelta(delta)) = msg.payload {
            seen.extend_from_slice(&delta.data);
        }
    }
}

/// The hex the probe printed between `GOT=` and `=END`.
fn probe_result(output: &[u8]) -> String {
    let text = String::from_utf8_lossy(output);
    let start = text.rfind("GOT=").expect("probe printed its result") + 4;
    let end = text[start..].find("=END").expect("probe result is terminated") + start;
    text[start..end].to_string()
}

async fn start_probe(sock: &std::path::Path) -> (TestClient, Vec<u8>, Vec<u8>) {
    let mut client = TestClient::connect(sock).await;
    client.handshake().await;
    let runtime_id =
        create_workspace(&mut client, "color-query", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client, &runtime_id).await;
    let pane_id = create_pane(&mut client, &runtime_id).await;
    // Let the shell finish starting before typing at it.
    client.drain(Duration::from_millis(500)).await;
    send_raw(&mut client, &runtime_id, &pane_id, format!("{PROBE}\n").as_bytes()).await;
    (client, runtime_id, pane_id)
}

#[tokio::test]
async fn color_reply_from_client_precedes_daemon_cursor_reply() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let (mut client, runtime_id, pane_id) = start_probe(&sock).await;

    // Play the terminal widget: the colour query arrives in the output
    // stream and is answered after a visible delay, as a real client would.
    let mut seen = Vec::new();
    output_until(&mut client, &mut seen, b"\x1b]11;?").await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    send_raw(&mut client, &runtime_id, &pane_id, COLOR_REPLY).await;

    output_until(&mut client, &mut seen, b"=END").await;
    let got = probe_result(&seen);
    let color_hex = COLOR_REPLY.iter().fold(String::new(), |mut hex, byte| {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
        hex
    });
    assert!(
        got.starts_with(&color_hex),
        "the colour reply must come first; the program read {got}"
    );
    assert!(
        got[color_hex.len()..].starts_with("1b5b"),
        "the cursor reply must follow the colour reply; the program read {got}"
    );
}

/// A client that never answers must not leave the program waiting: the
/// daemon's own reply is released after a short deadline.
#[tokio::test]
async fn cursor_reply_is_released_when_client_never_answers_color_query() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let (mut client, _runtime_id, _pane_id) = start_probe(&sock).await;

    let mut seen = Vec::new();
    output_until(&mut client, &mut seen, b"=END").await;
    let got = probe_result(&seen);
    assert!(got.starts_with("1b5b"), "expected only the cursor reply; the program read {got}");
}

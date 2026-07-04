//! Integration test: the daemon accepts only the v3 protocol.
//!
//! Regression for #980 Phase 2. A client whose first frame is not a valid
//! v3 `ClientHello` must be rejected: the
//! server sends no `ServerHello` and drops the connection.

mod common;

use bytes::BytesMut;
use common::start_test_server;
use rttx_proto::{decode_frame, encode_frame, v3};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn non_v3_first_frame_is_rejected() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut stream = tokio::net::UnixStream::connect(&sock).await.unwrap();

    // Send a length-prefixed frame that is NOT a valid v3 ClientHello — a
    // stand-in for an arbitrary non-v3 frame.
    let payload = b"non-v3 frame / arbitrary bytes";
    let mut frame = BytesMut::new();
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(payload);
    stream.write_all(&frame).await.unwrap();

    // The server must reject the connection: no ServerHello is sent and the
    // socket is closed (read returns EOF). Bounded so the test cannot hang.
    let mut read_buf = BytesMut::with_capacity(4096);
    let n = tokio::time::timeout(Duration::from_secs(5), stream.read_buf(&mut read_buf))
        .await
        .expect("server should close the connection promptly, not hang")
        .expect("read should not error");

    if n > 0 {
        // If any bytes came back, they must NOT be a successful ServerHello.
        assert!(
            decode_frame::<v3::ServerHello>(&mut read_buf).is_err(),
            "a rejected client must not receive a v3 ServerHello"
        );
    }
    // n == 0 (EOF) is the expected outcome: the connection was rejected.
}

#[tokio::test]
async fn v3_client_still_accepted_after_v2_removal() {
    // Sanity counterpart: a proper v3 handshake still succeeds.
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut stream = tokio::net::UnixStream::connect(&sock).await.unwrap();
    let hello = rttx_proto::v3_handshake::build_client_hello(
        uuid::Uuid::new_v4(),
        "test-client",
        "0.0.0",
        rttx_proto::v3_handshake::CORE_CAPABILITIES,
    );
    let mut buf = BytesMut::new();
    encode_frame(&hello, &mut buf).unwrap();
    stream.write_all(&buf).await.unwrap();

    let mut read_buf = BytesMut::with_capacity(4096);
    let server_hello = loop {
        if let Ok(sh) = decode_frame::<v3::ServerHello>(&mut read_buf) {
            break sh;
        }
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read_buf(&mut read_buf))
            .await
            .expect("v3 handshake should not hang")
            .expect("read should not error");
        assert!(n > 0, "v3 client must receive a ServerHello");
    };
    assert!(
        server_hello.negotiated_protocol_version >= 3,
        "negotiated protocol version should be v3 or higher"
    );
}

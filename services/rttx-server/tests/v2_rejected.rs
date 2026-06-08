//! Integration test: the daemon no longer supports the legacy v2 protocol.
//!
//! Regression for #980 Phase 2. A client that sends a v2 `ClientMessage` as
//! its first frame (instead of a v3 `ClientHello`) must be rejected: the
//! server sends no `ServerHello` and drops the connection.

mod common;

use bytes::BytesMut;
use common::start_test_server;
use rttx_proto::{decode_frame, encode_frame, proto, uuid_to_bytes, v3};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn v2_client_message_first_frame_is_rejected() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut stream = tokio::net::UnixStream::connect(&sock).await.unwrap();

    // Send a legacy v2 Hello (ClientMessage), which is no longer supported.
    let v2_hello = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Hello(proto::Hello {
            protocol_version: 2,
            client_id: uuid_to_bytes(uuid::Uuid::new_v4()),
        })),
    };
    let mut buf = BytesMut::new();
    encode_frame(&v2_hello, &mut buf).unwrap();
    stream.write_all(&buf).await.unwrap();

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
            "a v2 client must not receive a v3 ServerHello"
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

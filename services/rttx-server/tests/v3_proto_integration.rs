//! Integration test: v3 proto definitions compile and frame correctly.
//!
//! Verifies that the v3 protobuf types generated from `rttx-v3.proto` can be
//! used with the shared framing functions, and that the envelope structure
//! matches the RFC-021 command/response table.

use bytes::BytesMut;
use rttx_proto::{decode_frame, encode_frame, uuid_to_bytes, v3};

#[test]
fn v3_client_hello_frames_through_shared_codec() {
    let msg = v3::ClientHello {
        min_protocol_version: 3,
        max_protocol_version: 3,
        client_id: uuid_to_bytes(uuid::Uuid::new_v4()),
        client_name: "rttx".into(),
        client_version: env!("CARGO_PKG_VERSION").into(),
        capabilities: vec![
            v3::Capability::CoreRuntimeLifecycle as i32,
            v3::Capability::CorePaneLifecycle as i32,
            v3::Capability::CoreTerminalIo as i32,
            v3::Capability::CoreTerminalModes as i32,
            v3::Capability::CorePasteIntent as i32,
            v3::Capability::CoreFocusEvents as i32,
        ],
    };
    let mut buf = BytesMut::new();
    encode_frame(&msg, &mut buf).unwrap();
    let decoded: v3::ClientHello = decode_frame(&mut buf).unwrap();
    assert_eq!(msg.min_protocol_version, decoded.min_protocol_version);
    assert_eq!(msg.max_protocol_version, decoded.max_protocol_version);
    assert_eq!(msg.client_id, decoded.client_id);
    assert_eq!(msg.capabilities, decoded.capabilities);
}

#[test]
fn v3_envelope_roundtrip_create_runtime_and_response() {
    let request_id = 1;
    let cmd = v3::ClientEnvelope {
        request_id,
        command: Some(v3::client_envelope::Command::CreateRuntime(
            v3::CreateRuntime {
                name: "integration-test".into(),
                policy: v3::RuntimePolicy::Persistent as i32,
            },
        )),
    };
    let mut buf = BytesMut::new();
    encode_frame(&cmd, &mut buf).unwrap();
    let decoded: v3::ClientEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.request_id, request_id);
    assert!(matches!(
        decoded.command,
        Some(v3::client_envelope::Command::CreateRuntime(_))
    ));

    let runtime_id = uuid_to_bytes(uuid::Uuid::new_v4());
    let resp = v3::ServerEnvelope {
        request_id,
        payload: Some(v3::server_envelope::Payload::RuntimeCreated(
            v3::RuntimeCreated {
                runtime_id: runtime_id.clone(),
                runtime_revision: 1,
            },
        )),
    };
    let mut buf = BytesMut::new();
    encode_frame(&resp, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.request_id, request_id);
    if let Some(v3::server_envelope::Payload::RuntimeCreated(created)) = decoded.payload {
        assert_eq!(created.runtime_id, runtime_id);
        assert_eq!(created.runtime_revision, 1);
    } else {
        panic!("expected RuntimeCreated");
    }
}

#[test]
fn v3_protocol_error_frames_as_bare_and_in_envelope() {
    let err = v3::ProtocolError {
        kind: v3::ErrorKind::ProtocolMismatch as i32,
        message: "daemon supports v3, client requires v4".into(),
        operation: String::new(),
        retryable: false,
        user_action_required: true,
        retry_after_seconds: 0,
    };

    // Bare (handshake phase)
    let mut buf = BytesMut::new();
    encode_frame(&err, &mut buf).unwrap();
    let decoded: v3::ProtocolError = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.kind, v3::ErrorKind::ProtocolMismatch as i32);
    assert!(decoded.user_action_required);

    // Inside envelope (post-handshake)
    let env = v3::ServerEnvelope {
        request_id: 42,
        payload: Some(v3::server_envelope::Payload::Error(err)),
    };
    let mut buf = BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.request_id, 42);
    assert!(matches!(
        decoded.payload,
        Some(v3::server_envelope::Payload::Error(_))
    ));
}

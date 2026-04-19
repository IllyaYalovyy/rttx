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
    let runtime_name = "integration-test";
    let cmd = v3::ClientEnvelope {
        request_id,
        command: Some(v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
            name: runtime_name.into(),
            policy: v3::RuntimePolicy::Persistent as i32,
        })),
    };
    let mut buf = BytesMut::new();
    encode_frame(&cmd, &mut buf).unwrap();
    let decoded: v3::ClientEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.request_id, request_id);
    let Some(v3::client_envelope::Command::CreateRuntime(cr)) = decoded.command else {
        panic!("expected CreateRuntime");
    };
    assert_eq!(cr.name, runtime_name);

    let runtime_id = uuid_to_bytes(uuid::Uuid::new_v4());
    let resp = v3::ServerEnvelope {
        request_id,
        payload: Some(v3::server_envelope::Payload::RuntimeCreated(v3::RuntimeCreated {
            runtime_id: runtime_id.clone(),
            runtime_revision: 1,
        })),
    };
    let mut buf = BytesMut::new();
    encode_frame(&resp, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.request_id, request_id);
    let Some(v3::server_envelope::Payload::RuntimeCreated(created)) = decoded.payload else {
        panic!("expected RuntimeCreated");
    };
    assert_eq!(created.runtime_id, runtime_id);
    assert_eq!(created.runtime_revision, 1);
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
    let Some(v3::server_envelope::Payload::Error(_)) = decoded.payload else {
        panic!("expected ProtocolError");
    };
}

// ── V3 handshake integration tests ──

use rttx_proto::v3_handshake;

#[test]
fn v3_handshake_happy_path() {
    let client_id = uuid::Uuid::new_v4();
    let server_id = uuid::Uuid::new_v4();

    let mut caps = v3_handshake::CORE_CAPABILITIES.to_vec();
    caps.push(v3::Capability::OptDiagnostics);
    let client_hello = v3_handshake::build_client_hello(client_id, "rttx", "0.4.0", &caps);

    // Wire roundtrip
    let mut buf = BytesMut::new();
    encode_frame(&client_hello, &mut buf).unwrap();
    let decoded_hello: v3::ClientHello = decode_frame(&mut buf).unwrap();

    // Server negotiates version
    let negotiated = v3_handshake::negotiate_version(
        decoded_hello.min_protocol_version,
        decoded_hello.max_protocol_version,
        v3_handshake::V3_PROTOCOL_VERSION,
        v3_handshake::V3_PROTOCOL_VERSION,
    )
    .unwrap();
    assert_eq!(negotiated, 3);

    // Server builds hello with core caps only
    let server_hello = v3_handshake::build_server_hello(
        server_id,
        "0.4.0",
        negotiated,
        v3_handshake::CORE_CAPABILITIES,
    );

    // Wire roundtrip
    let mut buf = BytesMut::new();
    encode_frame(&server_hello, &mut buf).unwrap();
    let decoded_server: v3::ServerHello = decode_frame(&mut buf).unwrap();

    // Client validates server capabilities
    assert!(v3_handshake::validate_server_capabilities(&decoded_server.capabilities).is_ok());

    // Effective set is intersection (core only, since server has no optional)
    let effective =
        v3_handshake::effective_capabilities(&client_hello.capabilities, &decoded_server.capabilities);
    assert_eq!(effective.len(), 6);
    assert!(!effective.contains(&(v3::Capability::OptDiagnostics as i32)));
}

#[test]
fn v3_handshake_version_mismatch_sends_bare_error() {
    let client_hello = v3_handshake::build_client_hello(
        uuid::Uuid::new_v4(),
        "rttx",
        "0.5.0",
        v3_handshake::CORE_CAPABILITIES,
    );

    // Simulate a future server that only supports v5+
    let result = v3_handshake::negotiate_version(
        client_hello.min_protocol_version,
        client_hello.max_protocol_version,
        5,
        5,
    );
    let err = result.unwrap_err();

    // Error frames as bare message (not in envelope)
    let mut buf = BytesMut::new();
    encode_frame(&err, &mut buf).unwrap();
    let decoded: v3::ProtocolError = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.kind, v3::ErrorKind::ProtocolMismatch as i32);
    assert!(decoded.user_action_required);
}

#[test]
fn v3_handshake_missing_core_capability_rejected() {
    let server_id = uuid::Uuid::new_v4();
    // Server missing CORE_FOCUS_EVENTS
    let incomplete_caps: Vec<v3::Capability> = v3_handshake::CORE_CAPABILITIES
        .iter()
        .filter(|c| **c != v3::Capability::CoreFocusEvents)
        .copied()
        .collect();
    let server_hello =
        v3_handshake::build_server_hello(server_id, "0.3.0", 3, &incomplete_caps);

    let result = v3_handshake::validate_server_capabilities(&server_hello.capabilities);
    let missing = result.unwrap_err();
    assert_eq!(missing, vec![v3::Capability::CoreFocusEvents]);

    // Client builds error for the missing capabilities
    let err = v3_handshake::missing_capabilities_error(&missing);
    assert_eq!(err.kind, v3::ErrorKind::UnsupportedCapability as i32);
    assert!(err.message.contains("CORE_FOCUS_EVENTS"));

    // Error frames as bare message
    let mut buf = BytesMut::new();
    encode_frame(&err, &mut buf).unwrap();
    let decoded: v3::ProtocolError = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.kind, v3::ErrorKind::UnsupportedCapability as i32);
}

#[test]
fn v3_handshake_full_roundtrip_with_optional_capabilities() {
    let client_id = uuid::Uuid::new_v4();
    let server_id = uuid::Uuid::new_v4();

    // Both sides advertise all core + different optional sets
    let mut client_caps = v3_handshake::CORE_CAPABILITIES.to_vec();
    client_caps.push(v3::Capability::OptDiagnostics);
    client_caps.push(v3::Capability::OptResync);

    let mut server_caps = v3_handshake::CORE_CAPABILITIES.to_vec();
    server_caps.push(v3::Capability::OptResync);
    server_caps.push(v3::Capability::OptChunkedScrollback);

    let client_hello = v3_handshake::build_client_hello(client_id, "rttx", "0.4.0", &client_caps);
    let server_hello = v3_handshake::build_server_hello(server_id, "0.4.0", 3, &server_caps);

    let effective =
        v3_handshake::effective_capabilities(&client_hello.capabilities, &server_hello.capabilities);

    // Should have 6 core + OPT_RESYNC (shared), not OPT_DIAGNOSTICS or OPT_CHUNKED_SCROLLBACK
    assert_eq!(effective.len(), 7);
    assert!(effective.contains(&(v3::Capability::OptResync as i32)));
    assert!(!effective.contains(&(v3::Capability::OptDiagnostics as i32)));
    assert!(!effective.contains(&(v3::Capability::OptChunkedScrollback as i32)));
}

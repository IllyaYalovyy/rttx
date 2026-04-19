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

// ── V3 envelope correlation integration tests ──

use rttx_proto::v3_envelope;

#[test]
fn v3_envelope_request_response_correlation_roundtrip() {
    let id_gen = v3_envelope::RequestIdGenerator::new();
    let runtime_id = uuid_to_bytes(uuid::Uuid::new_v4());

    // Client sends CreateRuntime (request/response command)
    let cmd = v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
        name: "correlation-test".into(),
        policy: v3::RuntimePolicy::Persistent as i32,
    });
    let client_env = v3_envelope::build_client_envelope(&id_gen, cmd);
    assert_ne!(client_env.request_id, 0);
    let saved_request_id = client_env.request_id;

    // Wire roundtrip for client envelope
    let mut buf = BytesMut::new();
    encode_frame(&client_env, &mut buf).unwrap();
    let decoded_client: v3::ClientEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded_client.request_id, saved_request_id);

    // Server builds response echoing request_id
    let response = v3_envelope::build_response_envelope(
        decoded_client.request_id,
        v3::server_envelope::Payload::RuntimeCreated(v3::RuntimeCreated {
            runtime_id: runtime_id.clone(),
            runtime_revision: 1,
        }),
    );
    assert_eq!(response.request_id, saved_request_id);
    assert!(!v3_envelope::is_push_event(&response));

    // Wire roundtrip for server response
    let mut buf = BytesMut::new();
    encode_frame(&response, &mut buf).unwrap();
    let decoded_response: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded_response.request_id, saved_request_id);

    // Server sends a push event (OutputDelta)
    let push = v3_envelope::build_push_envelope(v3::server_envelope::Payload::OutputDelta(
        v3::OutputDelta {
            runtime_id,
            pane_id: uuid_to_bytes(uuid::Uuid::new_v4()),
            data: bytes::Bytes::from_static(b"hello"),
            pane_output_seq: 1,
        },
    ));
    assert!(v3_envelope::is_push_event(&push));

    let mut buf = BytesMut::new();
    encode_frame(&push, &mut buf).unwrap();
    let decoded_push: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded_push.request_id, 0);
    assert!(v3_envelope::is_push_event(&decoded_push));
}

#[test]
fn v3_envelope_fire_and_forget_skips_id_allocation() {
    let id_gen = v3_envelope::RequestIdGenerator::new();

    // Fire-and-forget: TerminalInput
    let cmd = v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
        runtime_id: uuid_to_bytes(uuid::Uuid::new_v4()),
        pane_id: uuid_to_bytes(uuid::Uuid::new_v4()),
        kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
            data: bytes::Bytes::from_static(b"ls\n"),
        })),
    });
    let env = v3_envelope::build_client_envelope(&id_gen, cmd);
    assert_eq!(env.request_id, 0);

    // Wire roundtrip
    let mut buf = BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();
    let decoded: v3::ClientEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.request_id, 0);

    // Next request/response command still gets ID 1
    let cmd = v3::client_envelope::Command::Ping(v3::Ping { nonce: 99 });
    let env = v3_envelope::build_client_envelope(&id_gen, cmd);
    assert_eq!(env.request_id, 1);
}

#[test]
fn v3_envelope_mixed_command_sequence() {
    let id_gen = v3_envelope::RequestIdGenerator::new();
    let rt = uuid_to_bytes(uuid::Uuid::new_v4());
    let pn = uuid_to_bytes(uuid::Uuid::new_v4());

    // Sequence: CreateRuntime(1), TerminalInput(0), Ping(2), ResizePane(0), ClosePane(3)
    let commands: Vec<v3::client_envelope::Command> = vec![
        v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
            name: "seq".into(),
            policy: v3::RuntimePolicy::Ephemeral as i32,
        }),
        v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
            runtime_id: rt.clone(),
            pane_id: pn.clone(),
            kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                data: bytes::Bytes::from_static(b"x"),
            })),
        }),
        v3::client_envelope::Command::Ping(v3::Ping { nonce: 1 }),
        v3::client_envelope::Command::ResizePane(v3::ResizePane {
            runtime_id: rt.clone(),
            pane_id: pn.clone(),
            cols: 80,
            rows: 24,
        }),
        v3::client_envelope::Command::ClosePane(v3::ClosePane { runtime_id: rt, pane_id: pn }),
    ];
    let expected_ids: Vec<u64> = vec![1, 0, 2, 0, 3];

    for (cmd, expected_id) in commands.into_iter().zip(expected_ids) {
        let env = v3_envelope::build_client_envelope(&id_gen, cmd);
        assert_eq!(env.request_id, expected_id);
    }
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
    let effective = v3_handshake::effective_capabilities(
        &client_hello.capabilities,
        &decoded_server.capabilities,
    );
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
    let server_hello = v3_handshake::build_server_hello(server_id, "0.3.0", 3, &incomplete_caps);

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

    let effective = v3_handshake::effective_capabilities(
        &client_hello.capabilities,
        &server_hello.capabilities,
    );

    // Should have 6 core + OPT_RESYNC (shared), not OPT_DIAGNOSTICS or OPT_CHUNKED_SCROLLBACK
    assert_eq!(effective.len(), 7);
    assert!(effective.contains(&(v3::Capability::OptResync as i32)));
    assert!(!effective.contains(&(v3::Capability::OptDiagnostics as i32)));
    assert!(!effective.contains(&(v3::Capability::OptChunkedScrollback as i32)));
}

// ── V3 terminal modes integration tests ──

use rttx_proto::v3_terminal_modes;

#[test]
fn v3_terminal_mode_changed_push_event_roundtrip() {
    let runtime_id = uuid::Uuid::new_v4();
    let pane_id = uuid::Uuid::new_v4();

    let modes = v3::TerminalModeState {
        bracketed_paste: true,
        focus_reporting: true,
        application_cursor_keys: true,
        application_keypad: true,
        alternate_screen: true,
        cursor_hidden: true,
        mouse_mode: v3::MouseMode::Any as i32,
        sgr_mouse: true,
    };

    let env = v3_terminal_modes::build_mode_changed_envelope(runtime_id, pane_id, 42, modes);
    assert!(v3_envelope::is_push_event(&env));

    // Wire roundtrip
    let mut buf = BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.request_id, 0);

    let Some(v3::server_envelope::Payload::TerminalModeChanged(changed)) = decoded.payload else {
        panic!("expected TerminalModeChanged");
    };
    assert_eq!(changed.runtime_id, uuid_to_bytes(runtime_id));
    assert_eq!(changed.pane_id, uuid_to_bytes(pane_id));
    assert_eq!(changed.runtime_revision, 42);
    let decoded_modes = changed.modes.expect("modes must be present");
    assert!(decoded_modes.bracketed_paste);
    assert!(decoded_modes.focus_reporting);
    assert!(decoded_modes.application_cursor_keys);
    assert!(decoded_modes.application_keypad);
    assert!(decoded_modes.alternate_screen);
    assert!(decoded_modes.cursor_hidden);
    assert_eq!(decoded_modes.mouse_mode, v3::MouseMode::Any as i32);
    assert!(decoded_modes.sgr_mouse);
}

#[test]
fn v3_mouse_mode_conversion_covers_all_tracking_values() {
    let cases: &[(u16, v3::MouseMode)] = &[
        (0, v3::MouseMode::None),
        (1000, v3::MouseMode::Normal),
        (1002, v3::MouseMode::Button),
        (1003, v3::MouseMode::Any),
        (9999, v3::MouseMode::None),
    ];
    for &(tracking, expected) in cases {
        let mode = v3_terminal_modes::mouse_mode_from_tracking_value(tracking);
        assert_eq!(mode, expected, "tracking value {tracking}");
        if tracking != 9999 {
            let back = v3_terminal_modes::tracking_value_from_mouse_mode(mode);
            assert_eq!(back, tracking, "roundtrip for tracking value {tracking}");
        }
    }
}

// ── V3 terminal input integration tests ──

use rttx_proto::v3_terminal_input;

#[test]
fn v3_structured_input_paste_with_bracketed_paste_active() {
    let runtime_id = uuid::Uuid::new_v4();
    let pane_id = uuid::Uuid::new_v4();

    let input = v3_terminal_input::build_paste_input(
        runtime_id,
        pane_id,
        bytes::Bytes::from_static(b"pasted content"),
    );

    // Wire roundtrip
    let mut buf = BytesMut::new();
    encode_frame(&input, &mut buf).unwrap();
    let decoded: v3::TerminalInput = decode_frame(&mut buf).unwrap();
    assert_eq!(input, decoded);

    // Resolve with bracketed paste active
    let modes = v3::TerminalModeState { bracketed_paste: true, ..Default::default() };
    let resolved = v3_terminal_input::resolve_input(decoded.kind.as_ref(), &modes);
    assert!(resolved.starts_with(b"\x1b[200~"));
    assert!(resolved.ends_with(b"\x1b[201~"));
    assert!(resolved.windows(b"pasted content".len()).any(|w| w == b"pasted content"));
}

#[test]
fn v3_structured_input_paste_without_bracketed_paste() {
    let input = v3_terminal_input::build_paste_input(
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        bytes::Bytes::from_static(b"plain paste"),
    );
    let modes = v3::TerminalModeState::default();
    let resolved = v3_terminal_input::resolve_input(input.kind.as_ref(), &modes);
    assert_eq!(resolved, b"plain paste");
}

#[test]
fn v3_structured_input_focus_in_with_reporting_active() {
    let input =
        v3_terminal_input::build_focus_input(uuid::Uuid::new_v4(), uuid::Uuid::new_v4(), true);
    let modes = v3::TerminalModeState { focus_reporting: true, ..Default::default() };
    let resolved = v3_terminal_input::resolve_input(input.kind.as_ref(), &modes);
    assert_eq!(resolved, b"\x1b[I");
}

#[test]
fn v3_structured_input_focus_out_with_reporting_active() {
    let input =
        v3_terminal_input::build_focus_input(uuid::Uuid::new_v4(), uuid::Uuid::new_v4(), false);
    let modes = v3::TerminalModeState { focus_reporting: true, ..Default::default() };
    let resolved = v3_terminal_input::resolve_input(input.kind.as_ref(), &modes);
    assert_eq!(resolved, b"\x1b[O");
}

#[test]
fn v3_structured_input_focus_suppressed_when_reporting_inactive() {
    let input =
        v3_terminal_input::build_focus_input(uuid::Uuid::new_v4(), uuid::Uuid::new_v4(), true);
    let modes = v3::TerminalModeState::default();
    let resolved = v3_terminal_input::resolve_input(input.kind.as_ref(), &modes);
    assert!(resolved.is_empty());
}

#[test]
fn v3_structured_input_raw_passthrough() {
    let input = v3_terminal_input::build_raw_input(
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        bytes::Bytes::from_static(b"\x1b[A"),
    );
    let modes = v3::TerminalModeState {
        bracketed_paste: true,
        focus_reporting: true,
        ..Default::default()
    };
    let resolved = v3_terminal_input::resolve_input(input.kind.as_ref(), &modes);
    assert_eq!(resolved, b"\x1b[A");
}

#[test]
fn v3_structured_input_envelope_fire_and_forget() {
    let input = v3_terminal_input::build_paste_input(
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        bytes::Bytes::from_static(b"text"),
    );
    let env = v3_terminal_input::build_terminal_input_envelope(input);
    assert_eq!(env.request_id, 0);

    // Wire roundtrip
    let mut buf = BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();
    let decoded: v3::ClientEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.request_id, 0);
    assert!(matches!(decoded.command, Some(v3::client_envelope::Command::TerminalInput(_))));
}

// ── v3_error integration tests ──

#[test]
fn v3_error_response_roundtrip_through_envelope() {
    use rttx_proto::v3_error;

    let err = v3_error::build_error(
        v3::ErrorKind::RuntimeNotFound,
        "runtime abc not found",
        "AttachRuntime",
    );
    let env = v3_error::build_error_response(42, err);
    assert_eq!(env.request_id, 42);

    let mut buf = BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.request_id, 42);
    match decoded.payload {
        Some(v3::server_envelope::Payload::Error(ref e)) => {
            assert_eq!(v3_error::error_kind(e), v3::ErrorKind::RuntimeNotFound);
            assert_eq!(e.operation, "AttachRuntime");
            assert!(!e.retryable);
        }
        _ => panic!("expected Error payload"),
    }
}

#[test]
fn v3_bare_protocol_error_during_handshake() {
    use rttx_proto::v3_error;

    let err = v3_error::build_error(
        v3::ErrorKind::ProtocolMismatch,
        "no common version: client v4–v4, server v3–v3",
        "Handshake",
    );
    assert!(err.user_action_required);
    assert!(!err.retryable);

    // Bare ProtocolError (not inside an envelope) — handshake phase
    let mut buf = BytesMut::new();
    encode_frame(&err, &mut buf).unwrap();
    let decoded: v3::ProtocolError = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.kind, v3::ErrorKind::ProtocolMismatch as i32);
    assert_eq!(decoded.operation, "Handshake");
}

#[test]
fn v3_error_classification_maps_to_connection_policy() {
    use rttx_proto::v3_error::{self, ErrorClassification};

    // Retryable errors → TransientError or StreamOverflow
    let overflow = v3_error::build_error(v3::ErrorKind::StreamOverflow, "overflow", "push");
    assert!(overflow.retryable);
    assert_eq!(
        v3_error::classify_error_kind(v3_error::error_kind(&overflow)),
        ErrorClassification::StreamOverflow
    );

    let internal = v3_error::build_error(v3::ErrorKind::Internal, "oops", "CreatePane");
    assert!(internal.retryable);
    assert_eq!(
        v3_error::classify_error_kind(v3_error::error_kind(&internal)),
        ErrorClassification::TransientError
    );

    // Non-retryable, user-action-required → IncompatibleVersion
    let mismatch = v3_error::build_error(v3::ErrorKind::ProtocolMismatch, "mismatch", "Handshake");
    assert!(!mismatch.retryable);
    assert!(mismatch.user_action_required);
    assert_eq!(
        v3_error::classify_error_kind(v3_error::error_kind(&mismatch)),
        ErrorClassification::IncompatibleVersion
    );

    // Ownership conflict
    let conflict = v3_error::build_error(v3::ErrorKind::OwnershipConflict, "busy", "AttachRuntime");
    assert_eq!(
        v3_error::classify_error_kind(v3_error::error_kind(&conflict)),
        ErrorClassification::OwnershipConflict
    );
}

#[test]
fn v3_error_unknown_kind_from_newer_server() {
    use rttx_proto::v3_error::{self, ErrorClassification};

    // Simulate a ProtocolError with an unknown ErrorKind value (from a newer server)
    let err = v3::ProtocolError {
        kind: 999,
        message: "future error kind".into(),
        operation: "FutureCommand".into(),
        retryable: true,
        user_action_required: false,
        retry_after_seconds: 10,
    };

    // Wire roundtrip preserves the raw i32 value
    let mut buf = BytesMut::new();
    encode_frame(&err, &mut buf).unwrap();
    let decoded: v3::ProtocolError = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.kind, 999);

    // error_kind() returns Unspecified for unknown values
    assert_eq!(v3_error::error_kind(&decoded), v3::ErrorKind::Unspecified);
    assert_eq!(
        v3_error::classify_error_kind(v3_error::error_kind(&decoded)),
        ErrorClassification::Unknown
    );
}

// ── V3 snapshot integration tests ──

use rttx_proto::v3_snapshot::{self, PaneSnapshotParams};

#[test]
fn v3_snapshot_attach_response_roundtrip() {
    let runtime_id = uuid::Uuid::new_v4();
    let pane_id = uuid::Uuid::new_v4();

    let pane = v3_snapshot::build_pane_snapshot(PaneSnapshotParams {
        pane_id,
        pane_output_seq: 42,
        title: "bash".into(),
        cwd: "/home/user".into(),
        cols: 120,
        rows: 40,
        exit_status: None,
        terminal_modes: v3::TerminalModeState { bracketed_paste: true, ..Default::default() },
        scrollback_tail: bytes::Bytes::from_static(b"$ ls\nfile.txt\n"),
        total_scrollback_bytes: 4096,
    });
    assert!(!pane.scrollback_complete);

    let snapshot = v3_snapshot::build_runtime_snapshot(
        runtime_id,
        10,
        v3::RuntimeClientRole::Writer,
        vec![pane],
    );
    let env = v3_snapshot::build_snapshot_response(7, snapshot);
    assert_eq!(env.request_id, 7);
    assert!(!v3_envelope::is_push_event(&env));

    // Wire roundtrip
    let mut buf = BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.request_id, 7);

    let Some(v3::server_envelope::Payload::RuntimeSnapshot(snap)) = decoded.payload else {
        panic!("expected RuntimeSnapshot");
    };
    assert_eq!(snap.runtime_id, uuid_to_bytes(runtime_id));
    assert_eq!(snap.runtime_revision, 10);
    assert_eq!(snap.client_role, v3::RuntimeClientRole::Writer as i32);
    assert_eq!(snap.panes.len(), 1);
    assert_eq!(snap.panes[0].pane_output_seq, 42);
    assert_eq!(snap.panes[0].scrollback_tail.as_ref(), b"$ ls\nfile.txt\n");
    assert!(!snap.panes[0].scrollback_complete);
    assert!(snap.panes[0].terminal_modes.as_ref().unwrap().bracketed_paste);
}

#[test]
fn v3_output_delta_sequence_continuity() {
    let runtime_id = uuid::Uuid::new_v4();
    let pane_id = uuid::Uuid::new_v4();

    // Simulate attach snapshot with pane_output_seq = 10
    let pane = v3_snapshot::build_pane_snapshot(PaneSnapshotParams {
        pane_id,
        pane_output_seq: 10,
        title: "zsh".into(),
        cwd: "/tmp".into(),
        cols: 80,
        rows: 24,
        exit_status: None,
        terminal_modes: v3::TerminalModeState::default(),
        scrollback_tail: bytes::Bytes::new(),
        total_scrollback_bytes: 0,
    });
    let mut expected_next = pane.pane_output_seq + 1;

    // Receive contiguous deltas 11, 12, 13
    for seq in [11, 12, 13] {
        let env = v3_snapshot::build_output_delta_envelope(
            runtime_id,
            pane_id,
            bytes::Bytes::from(format!("output-{seq}")),
            seq,
        );
        assert!(v3_envelope::is_push_event(&env));

        // Wire roundtrip
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
        let Some(v3::server_envelope::Payload::OutputDelta(delta)) = decoded.payload else {
            panic!("expected OutputDelta");
        };

        assert_eq!(v3_snapshot::detect_output_seq_gap(expected_next, delta.pane_output_seq), None);
        expected_next = delta.pane_output_seq + 1;
    }

    // Gap: receive seq 16 (skipped 14, 15)
    assert_eq!(v3_snapshot::detect_output_seq_gap(expected_next, 16), Some(2));
}

#[test]
fn v3_scrollback_truncation_and_snapshot() {
    let full_scrollback = vec![b'A'; 500_000];
    let (tail, complete) = v3_snapshot::truncate_scrollback(
        &full_scrollback,
        v3_snapshot::DEFAULT_SCROLLBACK_TAIL_LIMIT,
    );
    assert!(!complete);
    assert_eq!(tail.len(), v3_snapshot::DEFAULT_SCROLLBACK_TAIL_LIMIT);

    let pane = v3_snapshot::build_pane_snapshot(PaneSnapshotParams {
        pane_id: uuid::Uuid::new_v4(),
        pane_output_seq: 0,
        title: "bash".into(),
        cwd: "/".into(),
        cols: 80,
        rows: 24,
        exit_status: None,
        terminal_modes: v3::TerminalModeState::default(),
        scrollback_tail: tail,
        total_scrollback_bytes: full_scrollback.len() as u64,
    });
    assert!(!pane.scrollback_complete);
    assert_eq!(pane.total_scrollback_bytes, 500_000);

    // Wire roundtrip preserves all fields
    let mut buf = BytesMut::new();
    encode_frame(&pane, &mut buf).unwrap();
    let decoded: v3::PaneSnapshot = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.scrollback_tail.len(), v3_snapshot::DEFAULT_SCROLLBACK_TAIL_LIMIT);
    assert!(!decoded.scrollback_complete);
    assert_eq!(decoded.total_scrollback_bytes, 500_000);
}

// ── V3 chunked scrollback integration tests (OPT_CHUNKED_SCROLLBACK) ──

use rttx_proto::v3_scrollback;

#[test]
fn v3_chunked_scrollback_request_response_roundtrip() {
    let runtime_id = uuid::Uuid::new_v4();
    let pane_id = uuid::Uuid::new_v4();
    let id_gen = v3_envelope::RequestIdGenerator::new();

    // Client builds and sends GetScrollback request
    let req = v3_scrollback::build_get_scrollback(runtime_id, pane_id, 0, 65536);
    let client_env = v3_scrollback::build_get_scrollback_envelope(&id_gen, req);
    assert_ne!(client_env.request_id, 0);
    let saved_request_id = client_env.request_id;

    // Wire roundtrip for client envelope
    let mut buf = BytesMut::new();
    encode_frame(&client_env, &mut buf).unwrap();
    let decoded_client: v3::ClientEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded_client.request_id, saved_request_id);
    let Some(v3::client_envelope::Command::GetScrollback(gs)) = decoded_client.command else {
        panic!("expected GetScrollback command");
    };
    assert_eq!(gs.runtime_id, uuid_to_bytes(runtime_id));
    assert_eq!(gs.pane_id, uuid_to_bytes(pane_id));
    assert_eq!(gs.offset, 0);
    assert_eq!(gs.limit, 65536);

    // Server builds ScrollbackChunk response
    let chunk = v3_scrollback::build_scrollback_chunk(
        runtime_id,
        pane_id,
        0,
        bytes::Bytes::from_static(b"scrollback data"),
        true,
    );
    let server_env = v3_scrollback::build_scrollback_chunk_response(saved_request_id, chunk);
    assert_eq!(server_env.request_id, saved_request_id);
    assert!(!v3_envelope::is_push_event(&server_env));

    // Wire roundtrip for server envelope
    let mut buf = BytesMut::new();
    encode_frame(&server_env, &mut buf).unwrap();
    let decoded_server: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded_server.request_id, saved_request_id);
    let Some(v3::server_envelope::Payload::ScrollbackChunk(sc)) = decoded_server.payload else {
        panic!("expected ScrollbackChunk payload");
    };
    assert_eq!(sc.runtime_id, uuid_to_bytes(runtime_id));
    assert_eq!(sc.pane_id, uuid_to_bytes(pane_id));
    assert_eq!(sc.offset, 0);
    assert_eq!(sc.data.as_ref(), b"scrollback data");
    assert!(sc.is_last);
}

#[test]
fn v3_chunked_scrollback_paging_with_slice() {
    let scrollback = b"ABCDEFGHIJKLMNOP";
    let page_size: u32 = 4;
    let mut offset: u64 = 0;
    let mut collected = Vec::new();
    let mut pages = 0_u32;

    loop {
        let capped = v3_scrollback::cap_limit(page_size);
        let (data, is_last) = v3_scrollback::slice_scrollback(scrollback, offset, capped);
        collected.extend_from_slice(&data);
        offset += data.len() as u64;
        pages += 1;
        if is_last {
            break;
        }
    }

    assert_eq!(collected, scrollback);
    assert_eq!(pages, 4);
}

#[test]
fn v3_chunked_scrollback_capability_gating() {
    // With OPT_CHUNKED_SCROLLBACK negotiated
    let caps_with = vec![
        v3::Capability::CoreRuntimeLifecycle as i32,
        v3::Capability::OptChunkedScrollback as i32,
    ];
    assert!(v3_scrollback::is_supported(&caps_with));

    // Without OPT_CHUNKED_SCROLLBACK
    let caps_without = vec![v3::Capability::CoreRuntimeLifecycle as i32];
    assert!(!v3_scrollback::is_supported(&caps_without));

    // Server returns error when capability not negotiated
    let err = rttx_proto::v3_error::build_error(
        v3::ErrorKind::UnsupportedCapability,
        "OPT_CHUNKED_SCROLLBACK not negotiated",
        "GetScrollback",
    );
    let env = rttx_proto::v3_error::build_error_response(1, err);
    let mut buf = BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    let Some(v3::server_envelope::Payload::Error(e)) = decoded.payload else {
        panic!("expected Error payload");
    };
    assert_eq!(e.kind, v3::ErrorKind::UnsupportedCapability as i32);
    assert_eq!(e.operation, "GetScrollback");
}

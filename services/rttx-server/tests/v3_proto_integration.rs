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
            v3::Capability::CoreWorkspaceLifecycle as i32,
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
fn v3_envelope_roundtrip_create_workspace_and_response() {
    let request_id = 1;
    let workspace_name = "integration-test";
    let cmd = v3::ClientEnvelope {
        request_id,
        command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
            name: workspace_name.into(),
            policy: v3::WorkspacePolicy::Persistent as i32,
        })),
    };
    let mut buf = BytesMut::new();
    encode_frame(&cmd, &mut buf).unwrap();
    let decoded: v3::ClientEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.request_id, request_id);
    let Some(v3::client_envelope::Command::CreateWorkspace(cr)) = decoded.command else {
        panic!("expected CreateWorkspace");
    };
    assert_eq!(cr.name, workspace_name);

    let runtime_id = uuid_to_bytes(uuid::Uuid::new_v4());
    let resp = v3::ServerEnvelope {
        request_id,
        payload: Some(v3::server_envelope::Payload::WorkspaceCreated(v3::WorkspaceCreated {
            runtime_id: runtime_id.clone(),
            workspace_revision: 1,
        })),
    };
    let mut buf = BytesMut::new();
    encode_frame(&resp, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.request_id, request_id);
    let Some(v3::server_envelope::Payload::WorkspaceCreated(created)) = decoded.payload else {
        panic!("expected WorkspaceCreated");
    };
    assert_eq!(created.runtime_id, runtime_id);
    assert_eq!(created.workspace_revision, 1);
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

    // Client sends CreateWorkspace (request/response command)
    let cmd = v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
        name: "correlation-test".into(),
        policy: v3::WorkspacePolicy::Persistent as i32,
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
        v3::server_envelope::Payload::WorkspaceCreated(v3::WorkspaceCreated {
            runtime_id: runtime_id.clone(),
            workspace_revision: 1,
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

    // Sequence: CreateWorkspace(1), TerminalInput(0), Ping(2), ResizePane(0), ClosePane(3)
    let commands: Vec<v3::client_envelope::Command> = vec![
        v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
            name: "seq".into(),
            policy: v3::WorkspacePolicy::Ephemeral as i32,
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
    assert_eq!(changed.workspace_revision, 42);
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
        v3::ErrorKind::WorkspaceNotFound,
        "workspace abc not found",
        "AttachWorkspace",
    );
    let env = v3_error::build_error_response(42, err);
    assert_eq!(env.request_id, 42);

    let mut buf = BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.request_id, 42);
    match decoded.payload {
        Some(v3::server_envelope::Payload::Error(ref e)) => {
            assert_eq!(v3_error::error_kind(e), v3::ErrorKind::WorkspaceNotFound);
            assert_eq!(e.operation, "AttachWorkspace");
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
    let conflict =
        v3_error::build_error(v3::ErrorKind::OwnershipConflict, "busy", "AttachWorkspace");
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

    let snapshot = v3_snapshot::build_workspace_snapshot(
        runtime_id,
        10,
        v3::WorkspaceClientRole::Writer,
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

    let Some(v3::server_envelope::Payload::WorkspaceSnapshot(snap)) = decoded.payload else {
        panic!("expected WorkspaceSnapshot");
    };
    assert_eq!(snap.runtime_id, uuid_to_bytes(runtime_id));
    assert_eq!(snap.workspace_revision, 10);
    assert_eq!(snap.client_role, v3::WorkspaceClientRole::Writer as i32);
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
        v3::Capability::CoreWorkspaceLifecycle as i32,
        v3::Capability::OptChunkedScrollback as i32,
    ];
    assert!(v3_scrollback::is_supported(&caps_with));

    // Without OPT_CHUNKED_SCROLLBACK
    let caps_without = vec![v3::Capability::CoreWorkspaceLifecycle as i32];
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

// ── V3 resync integration tests (OPT_RESYNC) ──

use rttx_proto::v3_resync;

#[test]
fn v3_resync_overflow_and_resync_roundtrip() {
    let runtime_id = uuid::Uuid::new_v4();
    let pane_id = uuid::Uuid::new_v4();
    let id_gen = v3_envelope::RequestIdGenerator::new();

    // 1. Server sends StreamOverflow push event
    let overflow = v3_resync::build_stream_overflow(runtime_id, Some(pane_id), 5);
    let overflow_env = v3_resync::build_stream_overflow_envelope(overflow);
    assert!(v3_envelope::is_push_event(&overflow_env));

    // Wire roundtrip for overflow
    let mut buf = BytesMut::new();
    encode_frame(&overflow_env, &mut buf).unwrap();
    let decoded_overflow: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded_overflow.request_id, 0);
    let Some(v3::server_envelope::Payload::StreamOverflow(so)) = decoded_overflow.payload else {
        panic!("expected StreamOverflow");
    };
    assert_eq!(so.runtime_id, uuid_to_bytes(runtime_id));
    assert_eq!(so.pane_id, Some(uuid_to_bytes(pane_id)));
    assert_eq!(so.dropped_count, 5);

    // 2. Client sends ResyncWorkspace request
    let resync_req = v3_resync::build_resync_workspace(runtime_id);
    let resync_env = v3_resync::build_resync_workspace_envelope(&id_gen, resync_req);
    assert_ne!(resync_env.request_id, 0);
    let saved_request_id = resync_env.request_id;

    // Wire roundtrip for resync request
    let mut buf = BytesMut::new();
    encode_frame(&resync_env, &mut buf).unwrap();
    let decoded_resync: v3::ClientEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded_resync.request_id, saved_request_id);
    let Some(v3::client_envelope::Command::ResyncWorkspace(rs)) = decoded_resync.command else {
        panic!("expected ResyncWorkspace command");
    };
    assert_eq!(rs.runtime_id, uuid_to_bytes(runtime_id));

    // 3. Server responds with fresh WorkspaceSnapshot
    let pane = v3_snapshot::build_pane_snapshot(PaneSnapshotParams {
        pane_id,
        pane_output_seq: 200,
        title: "bash".into(),
        cwd: "/home/user".into(),
        cols: 120,
        rows: 40,
        exit_status: None,
        terminal_modes: v3::TerminalModeState::default(),
        scrollback_tail: bytes::Bytes::from_static(b"$ ls\n"),
        total_scrollback_bytes: 5,
    });
    let snapshot = v3_snapshot::build_workspace_snapshot(
        runtime_id,
        50,
        v3::WorkspaceClientRole::Writer,
        vec![pane],
    );
    let snap_env = v3_resync::build_resync_response(saved_request_id, snapshot);
    assert_eq!(snap_env.request_id, saved_request_id);
    assert!(!v3_envelope::is_push_event(&snap_env));

    // Wire roundtrip for snapshot response
    let mut buf = BytesMut::new();
    encode_frame(&snap_env, &mut buf).unwrap();
    let decoded_snap: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded_snap.request_id, saved_request_id);
    let Some(v3::server_envelope::Payload::WorkspaceSnapshot(snap)) = decoded_snap.payload else {
        panic!("expected WorkspaceSnapshot");
    };
    assert_eq!(snap.runtime_id, uuid_to_bytes(runtime_id));
    assert_eq!(snap.workspace_revision, 50);
    assert_eq!(snap.panes.len(), 1);
    assert_eq!(snap.panes[0].pane_output_seq, 200);
}

#[test]
fn v3_resync_capability_gating() {
    // With OPT_RESYNC negotiated
    let caps_with =
        vec![v3::Capability::CoreWorkspaceLifecycle as i32, v3::Capability::OptResync as i32];
    assert!(v3_resync::is_supported(&caps_with));

    // Without OPT_RESYNC
    let caps_without = vec![v3::Capability::CoreWorkspaceLifecycle as i32];
    assert!(!v3_resync::is_supported(&caps_without));

    // Server returns error when capability not negotiated
    let err = rttx_proto::v3_error::build_error(
        v3::ErrorKind::UnsupportedCapability,
        "OPT_RESYNC not negotiated",
        "ResyncWorkspace",
    );
    let env = rttx_proto::v3_error::build_error_response(1, err);
    let mut buf = BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    let Some(v3::server_envelope::Payload::Error(e)) = decoded.payload else {
        panic!("expected Error payload");
    };
    assert_eq!(e.kind, v3::ErrorKind::UnsupportedCapability as i32);
    assert_eq!(e.operation, "ResyncWorkspace");
}

#[test]
fn v3_resync_workspace_level_overflow() {
    // Workspace-level overflow (no pane_id)
    let runtime_id = uuid::Uuid::new_v4();
    let overflow = v3_resync::build_stream_overflow(runtime_id, None, 10);
    let env = v3_resync::build_stream_overflow_envelope(overflow);

    let mut buf = BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    let Some(v3::server_envelope::Payload::StreamOverflow(so)) = decoded.payload else {
        panic!("expected StreamOverflow");
    };
    assert_eq!(so.runtime_id, uuid_to_bytes(runtime_id));
    assert!(so.pane_id.is_none());
    assert_eq!(so.dropped_count, 10);
}

#[test]
fn v3_resync_seq_gap_triggers_resync_when_supported() {
    let id_gen = v3_envelope::RequestIdGenerator::new();
    let runtime_id = uuid::Uuid::new_v4();

    // Client detects gap in pane_output_seq
    let gap = v3_snapshot::detect_output_seq_gap(10, 15);
    assert_eq!(gap, Some(5));

    // Client has OPT_RESYNC → sends ResyncWorkspace
    let caps =
        vec![v3::Capability::CoreWorkspaceLifecycle as i32, v3::Capability::OptResync as i32];
    assert!(v3_resync::is_supported(&caps));

    let req = v3_resync::build_resync_workspace(runtime_id);
    let env = v3_resync::build_resync_workspace_envelope(&id_gen, req);
    assert_ne!(env.request_id, 0);

    // Wire roundtrip
    let mut buf = BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();
    let decoded: v3::ClientEnvelope = decode_frame(&mut buf).unwrap();
    let Some(v3::client_envelope::Command::ResyncWorkspace(rs)) = decoded.command else {
        panic!("expected ResyncWorkspace");
    };
    assert_eq!(rs.runtime_id, uuid_to_bytes(runtime_id));
}

#[test]
fn v3_without_resync_server_disconnects_client() {
    // Without OPT_RESYNC, server sends ProtocolError with StreamOverflow kind
    let caps = vec![v3::Capability::CoreWorkspaceLifecycle as i32];
    assert!(!v3_resync::is_supported(&caps));

    let err = rttx_proto::v3_error::build_error(
        v3::ErrorKind::StreamOverflow,
        "push channel overflow; client does not support OPT_RESYNC — disconnecting",
        "push",
    );
    assert!(err.retryable);

    let env = rttx_proto::v3_error::build_error_response(0, err);
    let mut buf = BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    let Some(v3::server_envelope::Payload::Error(e)) = decoded.payload else {
        panic!("expected Error payload");
    };
    assert_eq!(e.kind, v3::ErrorKind::StreamOverflow as i32);
    assert!(e.retryable);
}

// ── Capability profile matrix ──
//
// Issue #684: test latest GUI against each supported daemon capability profile.

use rttx_proto::{v3_diagnostics, v3_inventory, v3_takeover};

fn core_only_caps() -> Vec<i32> {
    v3_handshake::CORE_CAPABILITIES.iter().map(|c| *c as i32).collect()
}

fn all_optional_caps() -> Vec<v3::Capability> {
    vec![
        v3::Capability::OptWorkspaceInventory,
        v3::Capability::OptWorkspaceTakeover,
        v3::Capability::OptResync,
        v3::Capability::OptChunkedScrollback,
        v3::Capability::OptDiagnostics,
    ]
}

fn core_plus_all_optional_caps() -> Vec<i32> {
    let mut caps = core_only_caps();
    for opt in all_optional_caps() {
        caps.push(opt as i32);
    }
    caps
}

#[test]
fn v3_profile_core_only_rejects_all_optional() {
    let effective = core_only_caps();
    assert!(!v3_resync::is_supported(&effective));
    assert!(!v3_scrollback::is_supported(&effective));
    assert!(!v3_diagnostics::is_supported(&effective));
    assert!(!v3_inventory::is_supported(&effective));
    assert!(!v3_takeover::is_supported(&effective));
}

#[test]
fn v3_profile_core_plus_individual_optional() {
    for opt in all_optional_caps() {
        let mut effective = core_only_caps();
        effective.push(opt as i32);

        match opt {
            v3::Capability::OptResync => {
                assert!(v3_resync::is_supported(&effective));
                assert!(!v3_scrollback::is_supported(&effective));
                assert!(!v3_diagnostics::is_supported(&effective));
                assert!(!v3_inventory::is_supported(&effective));
                assert!(!v3_takeover::is_supported(&effective));
            }
            v3::Capability::OptChunkedScrollback => {
                assert!(!v3_resync::is_supported(&effective));
                assert!(v3_scrollback::is_supported(&effective));
                assert!(!v3_diagnostics::is_supported(&effective));
                assert!(!v3_inventory::is_supported(&effective));
                assert!(!v3_takeover::is_supported(&effective));
            }
            v3::Capability::OptDiagnostics => {
                assert!(!v3_resync::is_supported(&effective));
                assert!(!v3_scrollback::is_supported(&effective));
                assert!(v3_diagnostics::is_supported(&effective));
                assert!(!v3_inventory::is_supported(&effective));
                assert!(!v3_takeover::is_supported(&effective));
            }
            v3::Capability::OptWorkspaceInventory => {
                assert!(!v3_resync::is_supported(&effective));
                assert!(!v3_scrollback::is_supported(&effective));
                assert!(!v3_diagnostics::is_supported(&effective));
                assert!(v3_inventory::is_supported(&effective));
                assert!(!v3_takeover::is_supported(&effective));
            }
            v3::Capability::OptWorkspaceTakeover => {
                assert!(!v3_resync::is_supported(&effective));
                assert!(!v3_scrollback::is_supported(&effective));
                assert!(!v3_diagnostics::is_supported(&effective));
                assert!(!v3_inventory::is_supported(&effective));
                assert!(v3_takeover::is_supported(&effective));
            }
            _ => panic!("unexpected optional capability"),
        }
    }
}

#[test]
fn v3_profile_core_plus_all_optional() {
    let effective = core_plus_all_optional_caps();
    assert!(v3_resync::is_supported(&effective));
    assert!(v3_scrollback::is_supported(&effective));
    assert!(v3_diagnostics::is_supported(&effective));
    assert!(v3_inventory::is_supported(&effective));
    assert!(v3_takeover::is_supported(&effective));
}

#[test]
fn v3_profile_handshake_core_only_daemon() {
    let client_id = uuid::Uuid::new_v4();
    let server_id = uuid::Uuid::new_v4();

    let server_caps = core_only_caps();

    let client_hello = v3_handshake::build_client_hello(client_id, "rttx", "0.4.0", &{
        let mut caps: Vec<v3::Capability> = v3_handshake::CORE_CAPABILITIES.to_vec();
        caps.extend(all_optional_caps());
        caps
    });
    let server_hello =
        v3_handshake::build_server_hello(server_id, "0.4.0", 3, v3_handshake::CORE_CAPABILITIES);

    let effective = v3_handshake::effective_capabilities(
        &client_hello.capabilities,
        &server_hello.capabilities,
    );
    assert_eq!(effective.len(), 6);
    for opt in all_optional_caps() {
        assert!(!effective.contains(&(opt as i32)));
    }

    // Validate server has all core
    assert!(v3_handshake::validate_server_capabilities(&server_caps).is_ok());
}

#[test]
fn v3_profile_handshake_core_plus_all_optional_daemon() {
    let client_id = uuid::Uuid::new_v4();
    let server_id = uuid::Uuid::new_v4();

    let mut all_caps: Vec<v3::Capability> = v3_handshake::CORE_CAPABILITIES.to_vec();
    all_caps.extend(all_optional_caps());

    let client_hello = v3_handshake::build_client_hello(client_id, "rttx", "0.4.0", &all_caps);
    let server_hello = v3_handshake::build_server_hello(server_id, "0.4.0", 3, &all_caps);

    let effective = v3_handshake::effective_capabilities(
        &client_hello.capabilities,
        &server_hello.capabilities,
    );
    assert_eq!(effective.len(), 11); // 6 core + 5 optional
    for opt in all_optional_caps() {
        assert!(effective.contains(&(opt as i32)));
    }
}

// ── Send discipline: unnegotiated message gating ──

#[test]
fn v3_send_discipline_optional_client_commands_gated() {
    // Client must not send optional commands when capability is absent.
    let core_caps = core_only_caps();

    // ResyncWorkspace requires OPT_RESYNC
    assert!(!v3_resync::is_supported(&core_caps));

    // GetScrollback requires OPT_CHUNKED_SCROLLBACK
    assert!(!v3_scrollback::is_supported(&core_caps));

    // GetDiagnostics requires OPT_DIAGNOSTICS
    assert!(!v3_diagnostics::is_supported(&core_caps));

    // TakeoverWorkspace requires OPT_RUNTIME_TAKEOVER
    assert!(!v3_takeover::is_supported(&core_caps));

    // With each capability individually enabled, only that command is allowed
    let mut resync_caps = core_only_caps();
    resync_caps.push(v3::Capability::OptResync as i32);
    assert!(v3_resync::is_supported(&resync_caps));
    assert!(!v3_scrollback::is_supported(&resync_caps));

    let mut scrollback_caps = core_only_caps();
    scrollback_caps.push(v3::Capability::OptChunkedScrollback as i32);
    assert!(v3_scrollback::is_supported(&scrollback_caps));
    assert!(!v3_resync::is_supported(&scrollback_caps));
}

#[test]
fn v3_send_discipline_optional_server_payloads_gated() {
    // Server must not send optional payloads when capability is absent.
    let core_caps = core_only_caps();

    // StreamOverflow requires OPT_RESYNC
    assert!(!v3_resync::is_supported(&core_caps));

    // ScrollbackChunk requires OPT_CHUNKED_SCROLLBACK
    assert!(!v3_scrollback::is_supported(&core_caps));

    // DiagnosticsReport requires OPT_DIAGNOSTICS
    assert!(!v3_diagnostics::is_supported(&core_caps));

    // TakeoverCompleted/LeaseLost/OwnerDisconnected require OPT_RUNTIME_TAKEOVER
    assert!(!v3_takeover::is_supported(&core_caps));

    // WorkspaceList enriched fields require OPT_WORKSPACE_INVENTORY
    assert!(!v3_inventory::is_supported(&core_caps));
}

#[test]
fn v3_send_discipline_server_rejects_unnegotiated_command_with_error() {
    // When a client sends an optional command without the capability,
    // the server responds with UnsupportedCapability error.
    let id_gen = v3_envelope::RequestIdGenerator::new();

    // Client sends GetScrollback without OPT_CHUNKED_SCROLLBACK
    let cmd = v3::client_envelope::Command::GetScrollback(v3::GetScrollback {
        runtime_id: uuid_to_bytes(uuid::Uuid::new_v4()),
        pane_id: uuid_to_bytes(uuid::Uuid::new_v4()),
        offset: 0,
        limit: 65536,
    });
    let client_env = v3_envelope::build_client_envelope(&id_gen, cmd);

    // Server checks capability and builds error
    let core_caps = core_only_caps();
    assert!(!v3_scrollback::is_supported(&core_caps));
    let err = rttx_proto::v3_error::build_error(
        v3::ErrorKind::UnsupportedCapability,
        "OPT_CHUNKED_SCROLLBACK not negotiated",
        "GetScrollback",
    );
    let response = rttx_proto::v3_error::build_error_response(client_env.request_id, err);

    // Wire roundtrip
    let mut buf = BytesMut::new();
    encode_frame(&response, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.request_id, client_env.request_id);
    let Some(v3::server_envelope::Payload::Error(e)) = decoded.payload else {
        panic!("expected Error payload");
    };
    assert_eq!(e.kind, v3::ErrorKind::UnsupportedCapability as i32);
    assert_eq!(e.operation, "GetScrollback");
}

#[test]
fn v3_send_discipline_core_commands_always_allowed() {
    // Core commands are always valid regardless of optional capabilities.
    let core_caps = core_only_caps();
    assert!(v3_handshake::validate_server_capabilities(&core_caps).is_ok());

    // All core command types frame correctly
    let id_gen = v3_envelope::RequestIdGenerator::new();
    let rt = uuid_to_bytes(uuid::Uuid::new_v4());
    let pn = uuid_to_bytes(uuid::Uuid::new_v4());

    let core_commands: Vec<v3::client_envelope::Command> = vec![
        v3::client_envelope::Command::Ping(v3::Ping { nonce: 1 }),
        v3::client_envelope::Command::Shutdown(v3::Shutdown {}),
        v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
            name: "test".into(),
            policy: v3::WorkspacePolicy::Persistent as i32,
        }),
        v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
            runtime_id: rt.clone(),
            attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
        }),
        v3::client_envelope::Command::DetachWorkspace(v3::DetachWorkspace {
            runtime_id: rt.clone(),
        }),
        v3::client_envelope::Command::TerminateWorkspace(v3::TerminateWorkspace {
            runtime_id: rt.clone(),
        }),
        v3::client_envelope::Command::RenameWorkspace(v3::RenameWorkspace {
            runtime_id: rt.clone(),
            name: "renamed".into(),
        }),
        v3::client_envelope::Command::ListWorkspaces(v3::ListWorkspaces {}),
        v3::client_envelope::Command::CreatePane(v3::CreatePane {
            runtime_id: rt.clone(),
            cwd: Some("/tmp".into()),
            dark_background: Some(true),
            cols: 80,
            rows: 24,
            no_persist: None,
        }),
        v3::client_envelope::Command::ClosePane(v3::ClosePane {
            runtime_id: rt.clone(),
            pane_id: pn.clone(),
        }),
        v3::client_envelope::Command::ResizePane(v3::ResizePane {
            runtime_id: rt.clone(),
            pane_id: pn.clone(),
            cols: 120,
            rows: 40,
        }),
        v3::client_envelope::Command::SetPaneTitle(v3::SetPaneTitle {
            runtime_id: rt.clone(),
            pane_id: pn.clone(),
            title: "title".into(),
        }),
        v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
            runtime_id: rt,
            pane_id: pn,
            kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                data: bytes::Bytes::from_static(b"x"),
            })),
        }),
    ];

    for cmd in core_commands {
        let env = v3_envelope::build_client_envelope(&id_gen, cmd);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ClientEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }
}

// ── Wire compatibility: unknown enum values, unknown oneof variants, missing fields ──

#[test]
fn v3_wire_compat_unknown_enum_value_preserved_through_roundtrip() {
    // A ProtocolError with an unknown ErrorKind value (from a newer server)
    // must survive wire roundtrip with the raw i32 preserved.
    let err = v3::ProtocolError {
        kind: 999,
        message: "future error".into(),
        operation: "FutureOp".into(),
        retryable: true,
        user_action_required: false,
        retry_after_seconds: 30,
    };
    let mut buf = BytesMut::new();
    encode_frame(&err, &mut buf).unwrap();
    let decoded: v3::ProtocolError = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.kind, 999);
    assert_eq!(decoded.retry_after_seconds, 30);
}

#[test]
fn v3_wire_compat_unknown_capability_value_preserved() {
    // A ClientHello with a future capability value must preserve it.
    let hello = v3::ClientHello {
        min_protocol_version: 3,
        max_protocol_version: 3,
        client_id: uuid_to_bytes(uuid::Uuid::new_v4()),
        client_name: "rttx".into(),
        client_version: "0.5.0".into(),
        capabilities: vec![
            v3::Capability::CoreWorkspaceLifecycle as i32,
            200, // future optional capability
        ],
    };
    let mut buf = BytesMut::new();
    encode_frame(&hello, &mut buf).unwrap();
    let decoded: v3::ClientHello = decode_frame(&mut buf).unwrap();
    assert!(decoded.capabilities.contains(&200));
}

#[test]
fn v3_wire_compat_unknown_workspace_policy_value() {
    // A CreateWorkspace with an unknown policy value (from a newer client).
    let msg = v3::CreateWorkspace { name: "test".into(), policy: 99 };
    let mut buf = BytesMut::new();
    encode_frame(&msg, &mut buf).unwrap();
    let decoded: v3::CreateWorkspace = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.policy, 99);
    // try_from returns Unspecified for unknown values
    assert_eq!(
        v3::WorkspacePolicy::try_from(decoded.policy).unwrap_or(v3::WorkspacePolicy::Unspecified),
        v3::WorkspacePolicy::Unspecified
    );
}

#[test]
fn v3_wire_compat_unknown_mouse_mode_value() {
    let modes = v3::TerminalModeState { mouse_mode: 99, ..Default::default() };
    let mut buf = BytesMut::new();
    encode_frame(&modes, &mut buf).unwrap();
    let decoded: v3::TerminalModeState = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.mouse_mode, 99);
}

#[test]
fn v3_wire_compat_missing_optional_fields_default_to_zero_values() {
    // A PaneSnapshot with minimal fields — missing optional fields default.
    let minimal = v3::PaneSnapshot {
        pane_id: uuid_to_bytes(uuid::Uuid::new_v4()),
        pane_output_seq: 0,
        title: String::new(),
        cwd: String::new(),
        cols: 0,
        rows: 0,
        exit_status: None,
        terminal_modes: None,
        scrollback_tail: bytes::Bytes::new(),
        total_scrollback_bytes: 0,
        scrollback_complete: false,
    };
    let mut buf = BytesMut::new();
    encode_frame(&minimal, &mut buf).unwrap();
    let decoded: v3::PaneSnapshot = decode_frame(&mut buf).unwrap();
    assert!(decoded.terminal_modes.is_none());
    assert!(decoded.exit_status.is_none());
    assert!(decoded.scrollback_tail.is_empty());
}

#[test]
fn v3_wire_compat_empty_envelope_command_is_none() {
    // A ClientEnvelope with no command set (empty oneof).
    let env = v3::ClientEnvelope { request_id: 42, command: None };
    let mut buf = BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();
    let decoded: v3::ClientEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.request_id, 42);
    assert!(decoded.command.is_none());
}

#[test]
fn v3_wire_compat_empty_server_envelope_payload_is_none() {
    let env = v3::ServerEnvelope { request_id: 7, payload: None };
    let mut buf = BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(decoded.request_id, 7);
    assert!(decoded.payload.is_none());
}

#[test]
fn v3_wire_compat_extra_bytes_in_message_ignored_by_protobuf() {
    // Protobuf ignores unknown fields. Encode a Ping, then manually
    // construct a frame with extra field bytes appended.
    let msg = v3::Ping { nonce: 42 };
    let mut buf = BytesMut::new();
    encode_frame(&msg, &mut buf).unwrap();

    // Extract the raw protobuf payload (skip 4-byte length prefix)
    let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let payload = buf[4..4 + len].to_vec();

    // Append an unknown field (field 99, wire type 0 (varint): (99 << 3) | 0 = 792)
    let mut extended = payload;
    extended.extend_from_slice(&[0x98, 0x06, 0x00]);

    // Re-frame with updated length
    let mut new_buf = BytesMut::new();
    new_buf.extend_from_slice(&(extended.len() as u32).to_le_bytes());
    new_buf.extend_from_slice(&extended);

    let decoded: v3::Ping = decode_frame(&mut new_buf).unwrap();
    assert_eq!(decoded.nonce, 42);
}

#[test]
fn v3_wire_compat_unknown_oneof_variant_in_client_envelope() {
    // Encode a ClientEnvelope with no command, then append an unknown
    // oneof field to simulate a future client command.
    let env = v3::ClientEnvelope { request_id: 99, command: None };
    let mut buf = BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();

    let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let mut payload = buf[4..4 + len].to_vec();

    // Add unknown oneof field (field 200, wire type 2 = length-delimited)
    // (200 << 3) | 2 = 1602 → varint: 0xC2 0x0C
    payload.extend_from_slice(&[0xC2, 0x0C, 0x02, 0xAA, 0xBB]);

    let mut new_buf = BytesMut::new();
    new_buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    new_buf.extend_from_slice(&payload);

    let decoded: v3::ClientEnvelope = decode_frame(&mut new_buf).unwrap();
    assert_eq!(decoded.request_id, 99);
}

#[test]
fn v3_wire_compat_unknown_oneof_variant_in_server_envelope() {
    let env = v3::ServerEnvelope { request_id: 77, payload: None };
    let mut buf = BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();

    let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let mut payload = buf[4..4 + len].to_vec();

    // Add unknown oneof field (field 250, wire type 2)
    // (250 << 3) | 2 = 2002 → varint: 0xD2 0x0F
    payload.extend_from_slice(&[0xD2, 0x0F, 0x03, 0x01, 0x02, 0x03]);

    let mut new_buf = BytesMut::new();
    new_buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    new_buf.extend_from_slice(&payload);

    let decoded: v3::ServerEnvelope = decode_frame(&mut new_buf).unwrap();
    assert_eq!(decoded.request_id, 77);
}

#[test]
fn v3_wire_compat_workspace_info_without_enriched_fields() {
    // A WorkspaceInfo from a server without OPT_WORKSPACE_INVENTORY has
    // empty enriched fields (default values).
    let info = v3::WorkspaceInfo {
        id: uuid_to_bytes(uuid::Uuid::new_v4()),
        name: "test".into(),
        policy: v3::WorkspacePolicy::Persistent as i32,
        pane_count: 1,
        has_write_owner: true,
        read_only_client_count: 0,
        current_client_role: v3::WorkspaceClientRole::Writer as i32,
        workspace_revision: 5,
        reconstructed: false,
        // enriched fields left at defaults
        active_pane_summary: String::new(),
        takeover_eligible: false,
        disabled_reason: String::new(),
        panes: vec![],
    };
    let mut buf = BytesMut::new();
    encode_frame(&info, &mut buf).unwrap();
    let decoded: v3::WorkspaceInfo = decode_frame(&mut buf).unwrap();
    assert!(decoded.active_pane_summary.is_empty());
    assert!(decoded.panes.is_empty());
    assert!(!decoded.takeover_eligible);
}

// ── Core capabilities exercised end-to-end ──

#[test]
fn v3_core_workspace_lifecycle_end_to_end() {
    let id_gen = v3_envelope::RequestIdGenerator::new();
    let runtime_id = uuid_to_bytes(uuid::Uuid::new_v4());

    // CreateWorkspace → WorkspaceCreated
    let cmd = v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
        name: "lifecycle-test".into(),
        policy: v3::WorkspacePolicy::Persistent as i32,
    });
    let req = v3_envelope::build_client_envelope(&id_gen, cmd);
    let resp = v3_envelope::build_response_envelope(
        req.request_id,
        v3::server_envelope::Payload::WorkspaceCreated(v3::WorkspaceCreated {
            runtime_id: runtime_id.clone(),
            workspace_revision: 1,
        }),
    );
    assert_eq!(resp.request_id, req.request_id);

    // AttachWorkspace → WorkspaceSnapshot
    let cmd = v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
        runtime_id: runtime_id.clone(),
        attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
    });
    let req = v3_envelope::build_client_envelope(&id_gen, cmd);
    let resp = v3_envelope::build_response_envelope(
        req.request_id,
        v3::server_envelope::Payload::WorkspaceSnapshot(v3::WorkspaceSnapshot {
            tree: None,
            default_active_pane_id: Vec::new(),
            runtime_id: runtime_id.clone(),
            workspace_revision: 2,
            client_role: v3::WorkspaceClientRole::Writer as i32,
            panes: vec![],
        }),
    );
    assert_eq!(resp.request_id, req.request_id);

    // RenameWorkspace → WorkspaceRenamed
    let cmd = v3::client_envelope::Command::RenameWorkspace(v3::RenameWorkspace {
        runtime_id: runtime_id.clone(),
        name: "renamed".into(),
    });
    let req = v3_envelope::build_client_envelope(&id_gen, cmd);
    let resp = v3_envelope::build_response_envelope(
        req.request_id,
        v3::server_envelope::Payload::WorkspaceRenamed(v3::WorkspaceRenamed {
            runtime_id: runtime_id.clone(),
            name: "renamed".into(),
            workspace_revision: 3,
        }),
    );
    assert_eq!(resp.request_id, req.request_id);

    // DetachWorkspace → WorkspaceDetached
    let cmd = v3::client_envelope::Command::DetachWorkspace(v3::DetachWorkspace {
        runtime_id: runtime_id.clone(),
    });
    let req = v3_envelope::build_client_envelope(&id_gen, cmd);
    let resp = v3_envelope::build_response_envelope(
        req.request_id,
        v3::server_envelope::Payload::WorkspaceDetached(v3::WorkspaceDetached {
            runtime_id: runtime_id.clone(),
            workspace_revision: 4,
        }),
    );
    assert_eq!(resp.request_id, req.request_id);

    // TerminateWorkspace → WorkspaceTerminated
    let cmd = v3::client_envelope::Command::TerminateWorkspace(v3::TerminateWorkspace {
        runtime_id: runtime_id.clone(),
    });
    let req = v3_envelope::build_client_envelope(&id_gen, cmd);
    let resp = v3_envelope::build_response_envelope(
        req.request_id,
        v3::server_envelope::Payload::WorkspaceTerminated(v3::WorkspaceTerminated {
            runtime_id,
            final_revision: 5,
            reason: v3::WorkspaceTerminationReason::Explicit as i32,
        }),
    );
    assert_eq!(resp.request_id, req.request_id);

    // Wire roundtrip for final response
    let mut buf = BytesMut::new();
    encode_frame(&resp, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(resp, decoded);
}

#[test]
fn v3_core_pane_lifecycle_end_to_end() {
    let id_gen = v3_envelope::RequestIdGenerator::new();
    let runtime_id = uuid_to_bytes(uuid::Uuid::new_v4());
    let pane_id = uuid_to_bytes(uuid::Uuid::new_v4());

    // CreatePane → PaneCreated
    let cmd = v3::client_envelope::Command::CreatePane(v3::CreatePane {
        runtime_id: runtime_id.clone(),
        cwd: Some("/home/user".into()),
        dark_background: Some(true),
        cols: 80,
        rows: 24,
        no_persist: None,
    });
    let req = v3_envelope::build_client_envelope(&id_gen, cmd);
    assert_ne!(req.request_id, 0);
    let resp = v3_envelope::build_response_envelope(
        req.request_id,
        v3::server_envelope::Payload::PaneCreated(v3::PaneCreated {
            runtime_id: runtime_id.clone(),
            pane_id: pane_id.clone(),
            workspace_revision: 1,
        }),
    );
    assert_eq!(resp.request_id, req.request_id);

    // ResizePane (fire-and-forget) → PaneResized (push)
    let cmd = v3::client_envelope::Command::ResizePane(v3::ResizePane {
        runtime_id: runtime_id.clone(),
        pane_id: pane_id.clone(),
        cols: 120,
        rows: 40,
    });
    let req = v3_envelope::build_client_envelope(&id_gen, cmd);
    assert_eq!(req.request_id, 0);
    let push = v3_envelope::build_push_envelope(v3::server_envelope::Payload::PaneResized(
        v3::PaneResized {
            runtime_id: runtime_id.clone(),
            pane_id: pane_id.clone(),
            cols: 120,
            rows: 40,
            workspace_revision: 2,
        },
    ));
    assert!(v3_envelope::is_push_event(&push));

    // SetPaneTitle (fire-and-forget) → TitleChanged (push)
    let cmd = v3::client_envelope::Command::SetPaneTitle(v3::SetPaneTitle {
        runtime_id: runtime_id.clone(),
        pane_id: pane_id.clone(),
        title: "vim".into(),
    });
    let req = v3_envelope::build_client_envelope(&id_gen, cmd);
    assert_eq!(req.request_id, 0);

    // ClosePane → PaneClosed
    let cmd = v3::client_envelope::Command::ClosePane(v3::ClosePane {
        runtime_id: runtime_id.clone(),
        pane_id: pane_id.clone(),
    });
    let req = v3_envelope::build_client_envelope(&id_gen, cmd);
    assert_ne!(req.request_id, 0);
    let resp = v3_envelope::build_response_envelope(
        req.request_id,
        v3::server_envelope::Payload::PaneClosed(v3::PaneClosed {
            runtime_id,
            pane_id,
            workspace_revision: 3,
        }),
    );
    let mut buf = BytesMut::new();
    encode_frame(&resp, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(resp, decoded);
}

#[test]
fn v3_core_terminal_io_end_to_end() {
    let runtime_id = uuid::Uuid::new_v4();
    let pane_id = uuid::Uuid::new_v4();

    // TerminalInput (raw) → fire-and-forget
    let input = v3_terminal_input::build_raw_input(
        runtime_id,
        pane_id,
        bytes::Bytes::from_static(b"ls -la\n"),
    );
    let env = v3_terminal_input::build_terminal_input_envelope(input);
    assert_eq!(env.request_id, 0);

    // Server sends OutputDelta (push)
    let delta = v3_envelope::build_push_envelope(v3::server_envelope::Payload::OutputDelta(
        v3::OutputDelta {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
            data: bytes::Bytes::from_static(b"total 42\n"),
            pane_output_seq: 1,
        },
    ));
    assert!(v3_envelope::is_push_event(&delta));

    // Server sends CwdChanged (push)
    let cwd = v3_envelope::build_push_envelope(v3::server_envelope::Payload::CwdChanged(
        v3::CwdChanged {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
            cwd: "/home/user/project".into(),
            workspace_revision: 2,
        },
    ));
    assert!(v3_envelope::is_push_event(&cwd));

    // Server sends Bell (push)
    let bell = v3_envelope::build_push_envelope(v3::server_envelope::Payload::Bell(v3::Bell {
        runtime_id: uuid_to_bytes(runtime_id),
        pane_id: uuid_to_bytes(pane_id),
    }));
    assert!(v3_envelope::is_push_event(&bell));

    // Server sends PaneExited (push)
    let exited = v3_envelope::build_push_envelope(v3::server_envelope::Payload::PaneExited(
        v3::PaneExited {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
            status: 0,
            workspace_revision: 3,
        },
    ));
    assert!(v3_envelope::is_push_event(&exited));

    // Wire roundtrip for all push events
    for env in [delta, cwd, bell, exited] {
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }
}

#[test]
fn v3_core_terminal_modes_end_to_end() {
    let runtime_id = uuid::Uuid::new_v4();
    let pane_id = uuid::Uuid::new_v4();

    // All mode fields set
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
    let env = v3_terminal_modes::build_mode_changed_envelope(runtime_id, pane_id, 10, modes);
    assert!(v3_envelope::is_push_event(&env));

    let mut buf = BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    let Some(v3::server_envelope::Payload::TerminalModeChanged(changed)) = decoded.payload else {
        panic!("expected TerminalModeChanged");
    };
    let m = changed.modes.unwrap();
    assert!(m.bracketed_paste);
    assert!(m.focus_reporting);
    assert!(m.application_cursor_keys);
    assert!(m.application_keypad);
    assert!(m.alternate_screen);
    assert!(m.cursor_hidden);
    assert_eq!(m.mouse_mode, v3::MouseMode::Any as i32);
    assert!(m.sgr_mouse);
}

#[test]
fn v3_core_paste_intent_end_to_end() {
    let runtime_id = uuid::Uuid::new_v4();
    let pane_id = uuid::Uuid::new_v4();

    // Paste with bracketed paste active
    let input = v3_terminal_input::build_paste_input(
        runtime_id,
        pane_id,
        bytes::Bytes::from_static(b"pasted text"),
    );
    let modes = v3::TerminalModeState { bracketed_paste: true, ..Default::default() };
    let resolved = v3_terminal_input::resolve_input(input.kind.as_ref(), &modes);
    assert!(resolved.starts_with(b"\x1b[200~"));
    assert!(resolved.ends_with(b"\x1b[201~"));

    // Paste without bracketed paste
    let modes_off = v3::TerminalModeState::default();
    let resolved_off = v3_terminal_input::resolve_input(input.kind.as_ref(), &modes_off);
    assert_eq!(resolved_off, b"pasted text");
}

#[test]
fn v3_core_focus_events_end_to_end() {
    let runtime_id = uuid::Uuid::new_v4();
    let pane_id = uuid::Uuid::new_v4();

    // Focus in with reporting active
    let focus_in = v3_terminal_input::build_focus_input(runtime_id, pane_id, true);
    let modes_on = v3::TerminalModeState { focus_reporting: true, ..Default::default() };
    assert_eq!(v3_terminal_input::resolve_input(focus_in.kind.as_ref(), &modes_on), b"\x1b[I");

    // Focus out with reporting active
    let focus_out = v3_terminal_input::build_focus_input(runtime_id, pane_id, false);
    assert_eq!(v3_terminal_input::resolve_input(focus_out.kind.as_ref(), &modes_on), b"\x1b[O");

    // Focus events suppressed when reporting inactive
    let modes_off = v3::TerminalModeState::default();
    assert!(v3_terminal_input::resolve_input(focus_in.kind.as_ref(), &modes_off).is_empty());
    assert!(v3_terminal_input::resolve_input(focus_out.kind.as_ref(), &modes_off).is_empty());
}

// ── Optional capability absent fallback paths ──

#[test]
fn v3_opt_enriched_inventory_absent_strips_extended_fields() {
    let effective = core_only_caps();
    assert!(!v3_inventory::is_supported(&effective));

    // Server builds WorkspaceInfo with only core fields
    let info = v3::WorkspaceInfo {
        id: uuid_to_bytes(uuid::Uuid::new_v4()),
        name: "test".into(),
        policy: v3::WorkspacePolicy::Persistent as i32,
        pane_count: 2,
        has_write_owner: true,
        read_only_client_count: 0,
        current_client_role: v3::WorkspaceClientRole::Writer as i32,
        workspace_revision: 5,
        reconstructed: false,
        active_pane_summary: String::new(),
        takeover_eligible: false,
        disabled_reason: String::new(),
        panes: vec![],
    };
    let mut buf = BytesMut::new();
    encode_frame(&info, &mut buf).unwrap();
    let decoded: v3::WorkspaceInfo = decode_frame(&mut buf).unwrap();
    assert!(decoded.panes.is_empty());
    assert!(decoded.active_pane_summary.is_empty());
}

#[test]
fn v3_opt_takeover_absent_attach_blocked_without_takeover() {
    let effective = core_only_caps();
    assert!(!v3_takeover::is_supported(&effective));

    // Without OPT_RUNTIME_TAKEOVER, server sends AttachBlocked
    let blocked = v3_envelope::build_push_envelope(v3::server_envelope::Payload::AttachBlocked(
        v3::AttachBlocked {
            runtime_id: uuid_to_bytes(uuid::Uuid::new_v4()),
            current_client_role: v3::WorkspaceClientRole::Unattached as i32,
            attached_client_count: 1,
            read_only_client_count: 0,
        },
    ));
    let mut buf = BytesMut::new();
    encode_frame(&blocked, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert!(matches!(decoded.payload, Some(v3::server_envelope::Payload::AttachBlocked(_))));
}

#[test]
fn v3_opt_diagnostics_absent_returns_unsupported_error() {
    let effective = core_only_caps();
    assert!(!v3_diagnostics::is_supported(&effective));

    let err = rttx_proto::v3_error::build_error(
        v3::ErrorKind::UnsupportedCapability,
        "OPT_DIAGNOSTICS not negotiated",
        "GetDiagnostics",
    );
    let env = rttx_proto::v3_error::build_error_response(1, err);
    let mut buf = BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    let Some(v3::server_envelope::Payload::Error(e)) = decoded.payload else {
        panic!("expected Error");
    };
    assert_eq!(e.kind, v3::ErrorKind::UnsupportedCapability as i32);
    assert_eq!(e.operation, "GetDiagnostics");
}

// ── Backpressure: forced disconnect without OPT_RESYNC ──

#[test]
fn v3_backpressure_disconnect_without_resync_is_retryable() {
    let effective = core_only_caps();
    assert!(!v3_resync::is_supported(&effective));

    let err = rttx_proto::v3_error::build_error(
        v3::ErrorKind::StreamOverflow,
        "push channel overflow; OPT_RESYNC not negotiated — disconnecting",
        "push",
    );
    assert!(err.retryable);
    assert!(!err.user_action_required);

    let classification =
        rttx_proto::v3_error::classify_error_kind(rttx_proto::v3_error::error_kind(&err));
    assert_eq!(classification, rttx_proto::v3_error::ErrorClassification::StreamOverflow);
}

// ── Typed error mapping completeness ──

#[test]
fn v3_error_all_kinds_map_to_connection_problem() {
    use rttx_proto::v3_error::{ErrorClassification, classify_error_kind};

    let mappings: &[(v3::ErrorKind, ErrorClassification)] = &[
        (v3::ErrorKind::Unspecified, ErrorClassification::Unknown),
        (v3::ErrorKind::ProtocolMismatch, ErrorClassification::IncompatibleVersion),
        (v3::ErrorKind::UnsupportedCapability, ErrorClassification::IncompatibleVersion),
        (v3::ErrorKind::InvalidArgument, ErrorClassification::InvalidRequest),
        (v3::ErrorKind::WorkspaceNotFound, ErrorClassification::ResourceNotFound),
        (v3::ErrorKind::PaneNotFound, ErrorClassification::ResourceNotFound),
        (v3::ErrorKind::OwnershipConflict, ErrorClassification::OwnershipConflict),
        (v3::ErrorKind::TakeoverRequired, ErrorClassification::OwnershipConflict),
        (v3::ErrorKind::StreamOverflow, ErrorClassification::StreamOverflow),
        (v3::ErrorKind::Internal, ErrorClassification::TransientError),
    ];
    for &(kind, expected) in mappings {
        assert_eq!(classify_error_kind(kind), expected, "ErrorKind::{kind:?}");
    }
}

#[test]
fn v3_error_each_kind_roundtrips_through_envelope() {
    let kinds = [
        v3::ErrorKind::Unspecified,
        v3::ErrorKind::ProtocolMismatch,
        v3::ErrorKind::UnsupportedCapability,
        v3::ErrorKind::InvalidArgument,
        v3::ErrorKind::WorkspaceNotFound,
        v3::ErrorKind::PaneNotFound,
        v3::ErrorKind::OwnershipConflict,
        v3::ErrorKind::TakeoverRequired,
        v3::ErrorKind::StreamOverflow,
        v3::ErrorKind::Internal,
    ];
    for kind in kinds {
        let err = rttx_proto::v3_error::build_error(kind, "test message", "TestOp");
        let env = rttx_proto::v3_error::build_error_response(42, err);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
        let Some(v3::server_envelope::Payload::Error(e)) = decoded.payload else {
            panic!("expected Error for {kind:?}");
        };
        assert_eq!(rttx_proto::v3_error::error_kind(&e), kind);
    }
}

// ── Ping/Pong control flow ──

#[test]
fn v3_ping_pong_roundtrip() {
    let id_gen = v3_envelope::RequestIdGenerator::new();
    let cmd = v3::client_envelope::Command::Ping(v3::Ping { nonce: 12345 });
    let req = v3_envelope::build_client_envelope(&id_gen, cmd);
    assert_ne!(req.request_id, 0);

    let resp = v3_envelope::build_response_envelope(
        req.request_id,
        v3::server_envelope::Payload::Pong(v3::Pong { nonce: 12345 }),
    );
    assert_eq!(resp.request_id, req.request_id);

    let mut buf = BytesMut::new();
    encode_frame(&resp, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    let Some(v3::server_envelope::Payload::Pong(pong)) = decoded.payload else {
        panic!("expected Pong");
    };
    assert_eq!(pong.nonce, 12345);
}

// ── ListWorkspaces end-to-end ──

#[test]
fn v3_list_workspaces_end_to_end() {
    let id_gen = v3_envelope::RequestIdGenerator::new();
    let cmd = v3::client_envelope::Command::ListWorkspaces(v3::ListWorkspaces {});
    let req = v3_envelope::build_client_envelope(&id_gen, cmd);
    assert_ne!(req.request_id, 0);

    let runtime_id = uuid_to_bytes(uuid::Uuid::new_v4());
    let resp = v3_envelope::build_response_envelope(
        req.request_id,
        v3::server_envelope::Payload::WorkspaceList(v3::WorkspaceList {
            workspaces: vec![v3::WorkspaceInfo {
                id: runtime_id,
                name: "dev".into(),
                policy: v3::WorkspacePolicy::Persistent as i32,
                pane_count: 2,
                has_write_owner: true,
                read_only_client_count: 0,
                current_client_role: v3::WorkspaceClientRole::Unattached as i32,
                workspace_revision: 10,
                reconstructed: false,
                active_pane_summary: String::new(),
                takeover_eligible: false,
                disabled_reason: String::new(),
                panes: vec![],
            }],
        }),
    );

    let mut buf = BytesMut::new();
    encode_frame(&resp, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    let Some(v3::server_envelope::Payload::WorkspaceList(list)) = decoded.payload else {
        panic!("expected WorkspaceList");
    };
    assert_eq!(list.workspaces.len(), 1);
    assert_eq!(list.workspaces[0].name, "dev");
}

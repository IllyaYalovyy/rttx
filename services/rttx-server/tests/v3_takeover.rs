//! Integration tests for v3 `OPT_RUNTIME_TAKEOVER` protocol builders.
//!
//! Validates the end-to-end flow: build takeover request, receive lease
//! events, gate on capability, and verify wire roundtrip through envelopes.

use rttx_proto::v3;
use rttx_proto::v3_envelope::RequestIdGenerator;
use rttx_proto::v3_takeover;
use rttx_proto::{decode_frame, encode_frame, uuid_to_bytes};

fn rt() -> uuid::Uuid {
    uuid::Uuid::new_v4()
}

fn client() -> uuid::Uuid {
    uuid::Uuid::new_v4()
}

#[test]
fn v3_takeover_full_flow_wire_roundtrip() {
    let id_gen = RequestIdGenerator::new();
    let r = rt();
    let new_owner = client();

    // 1. Client sends TakeoverRuntime request
    let req = v3_takeover::build_takeover_runtime(r);
    let req_env = v3_takeover::build_takeover_runtime_envelope(&id_gen, req);
    let mut buf = bytes::BytesMut::new();
    encode_frame(&req_env, &mut buf).unwrap();
    let decoded_req: v3::ClientEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(req_env, decoded_req);
    assert_ne!(decoded_req.request_id, 0);

    // 2. Server sends LeaseLost push event to previous writer
    let lost = v3_takeover::build_lease_lost(r, 50, new_owner);
    let lost_env = v3_takeover::build_lease_lost_envelope(lost);
    let mut buf = bytes::BytesMut::new();
    encode_frame(&lost_env, &mut buf).unwrap();
    let decoded_lost: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(lost_env, decoded_lost);
    assert_eq!(decoded_lost.request_id, 0);

    // 3. Server responds with TakeoverCompleted
    let completed = v3_takeover::build_takeover_completed(r, 51);
    let resp_env =
        v3_takeover::build_takeover_completed_response(decoded_req.request_id, completed);
    let mut buf = bytes::BytesMut::new();
    encode_frame(&resp_env, &mut buf).unwrap();
    let decoded_resp: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(resp_env, decoded_resp);
    assert_eq!(decoded_resp.request_id, decoded_req.request_id);

    // Verify payload contents
    let v3::server_envelope::Payload::TakeoverCompleted(ref tc) = decoded_resp.payload.unwrap()
    else {
        panic!("expected TakeoverCompleted payload");
    };
    assert_eq!(tc.runtime_id, uuid_to_bytes(r));
    assert_eq!(tc.runtime_revision, 51);
}

#[test]
fn v3_takeover_capability_gating_rejects_when_absent() {
    let caps_without_takeover =
        vec![v3::Capability::CoreRuntimeLifecycle as i32, v3::Capability::CorePaneLifecycle as i32];
    assert!(!v3_takeover::is_supported(&caps_without_takeover));

    // Server returns OwnershipConflict error instead of allowing takeover
    let err = rttx_proto::v3_error::build_error(
        v3::ErrorKind::OwnershipConflict,
        "runtime owned by another client; takeover not available",
        "AttachRuntime",
    );
    let env = rttx_proto::v3_error::build_error_response(1, err);
    let mut buf = bytes::BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    let v3::server_envelope::Payload::Error(ref e) = decoded.payload.unwrap() else {
        panic!("expected Error payload");
    };
    assert_eq!(e.kind, v3::ErrorKind::OwnershipConflict as i32);
    assert!(!e.retryable);
}

#[test]
fn v3_takeover_owner_disconnected_notifies_readers() {
    let r = rt();

    let disconnected = v3_takeover::build_owner_disconnected(r, 60);
    let env = v3_takeover::build_owner_disconnected_envelope(disconnected);
    let mut buf = bytes::BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(env, decoded);
    assert_eq!(decoded.request_id, 0);

    let v3::server_envelope::Payload::OwnerDisconnected(ref od) = decoded.payload.unwrap() else {
        panic!("expected OwnerDisconnected payload");
    };
    assert_eq!(od.runtime_id, uuid_to_bytes(r));
    assert_eq!(od.runtime_revision, 60);
}

#[test]
fn v3_takeover_attach_blocked_then_takeover_flow() {
    let id_gen = RequestIdGenerator::new();
    let r = rt();

    // 1. Attach is blocked
    let blocked = v3::AttachBlocked {
        runtime_id: uuid_to_bytes(r),
        current_client_role: v3::RuntimeClientRole::Unattached as i32,
        attached_client_count: 1,
        read_only_client_count: 0,
    };
    let blocked_env = rttx_proto::v3_envelope::build_response_envelope(
        1,
        v3::server_envelope::Payload::AttachBlocked(blocked),
    );
    let mut buf = bytes::BytesMut::new();
    encode_frame(&blocked_env, &mut buf).unwrap();
    let decoded_blocked: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(blocked_env, decoded_blocked);

    // 2. Client has OPT_RUNTIME_TAKEOVER, sends takeover
    let caps = vec![
        v3::Capability::CoreRuntimeLifecycle as i32,
        v3::Capability::OptRuntimeTakeover as i32,
    ];
    assert!(v3_takeover::is_supported(&caps));

    let req = v3_takeover::build_takeover_runtime(r);
    let req_env = v3_takeover::build_takeover_runtime_envelope(&id_gen, req);

    // 3. Server responds with TakeoverCompleted
    let completed = v3_takeover::build_takeover_completed(r, 70);
    let resp = v3_takeover::build_takeover_completed_response(req_env.request_id, completed);
    let mut buf = bytes::BytesMut::new();
    encode_frame(&resp, &mut buf).unwrap();
    let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
    assert_eq!(resp, decoded);
    assert_eq!(decoded.request_id, req_env.request_id);
}

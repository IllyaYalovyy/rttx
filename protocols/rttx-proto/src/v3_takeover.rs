//! V3 runtime takeover: capability gating, builders, and lease events.
//!
//! Implements RFC-021 Section 10 (`OPT_RUNTIME_TAKEOVER`).
//!
//! Takeover is an explicit command, not a side effect of attach. One writer
//! lease per runtime, zero or more readers. Without `OPT_RUNTIME_TAKEOVER`,
//! the client shows "session busy" without a takeover option.
//!
//! Lease events:
//! - `TakeoverCompleted` — response to the requesting client on success
//! - `LeaseLost` — push event to the previous writer when their lease is taken
//! - `OwnerDisconnected` — push event to readers when the writer disconnects

use crate::v3;

/// Check whether `OPT_RUNTIME_TAKEOVER` is in the effective capability set.
#[must_use]
pub fn is_supported(effective_caps: &[i32]) -> bool {
    effective_caps.contains(&(v3::Capability::OptRuntimeTakeover as i32))
}

/// Build a `TakeoverRuntime` request.
#[must_use]
pub fn build_takeover_runtime(runtime_id: uuid::Uuid) -> v3::TakeoverRuntime {
    v3::TakeoverRuntime { runtime_id: crate::uuid_to_bytes(runtime_id) }
}

/// Build a `ClientEnvelope` for a `TakeoverRuntime` request.
#[must_use]
pub fn build_takeover_runtime_envelope(
    id_gen: &crate::v3_envelope::RequestIdGenerator,
    request: v3::TakeoverRuntime,
) -> v3::ClientEnvelope {
    crate::v3_envelope::build_client_envelope(
        id_gen,
        v3::client_envelope::Command::TakeoverRuntime(request),
    )
}

/// Build a `TakeoverCompleted` response.
#[must_use]
pub fn build_takeover_completed(
    runtime_id: uuid::Uuid,
    runtime_revision: u64,
) -> v3::TakeoverCompleted {
    v3::TakeoverCompleted {
        runtime_id: crate::uuid_to_bytes(runtime_id),
        runtime_revision,
    }
}

/// Build a `ServerEnvelope` response containing a `TakeoverCompleted`.
#[must_use]
pub fn build_takeover_completed_response(
    request_id: u64,
    completed: v3::TakeoverCompleted,
) -> v3::ServerEnvelope {
    crate::v3_envelope::build_response_envelope(
        request_id,
        v3::server_envelope::Payload::TakeoverCompleted(completed),
    )
}

/// Build a `LeaseLost` push event sent to the previous writer.
#[must_use]
pub fn build_lease_lost(
    runtime_id: uuid::Uuid,
    runtime_revision: u64,
    new_owner_id: uuid::Uuid,
) -> v3::LeaseLost {
    v3::LeaseLost {
        runtime_id: crate::uuid_to_bytes(runtime_id),
        runtime_revision,
        new_owner_id: crate::uuid_to_bytes(new_owner_id),
    }
}

/// Build a `ServerEnvelope` push event containing a `LeaseLost`.
#[must_use]
pub fn build_lease_lost_envelope(lease_lost: v3::LeaseLost) -> v3::ServerEnvelope {
    crate::v3_envelope::build_push_envelope(v3::server_envelope::Payload::LeaseLost(lease_lost))
}

/// Build an `OwnerDisconnected` push event sent to readers.
#[must_use]
pub fn build_owner_disconnected(
    runtime_id: uuid::Uuid,
    runtime_revision: u64,
) -> v3::OwnerDisconnected {
    v3::OwnerDisconnected {
        runtime_id: crate::uuid_to_bytes(runtime_id),
        runtime_revision,
    }
}

/// Build a `ServerEnvelope` push event containing an `OwnerDisconnected`.
#[must_use]
pub fn build_owner_disconnected_envelope(
    disconnected: v3::OwnerDisconnected,
) -> v3::ServerEnvelope {
    crate::v3_envelope::build_push_envelope(v3::server_envelope::Payload::OwnerDisconnected(
        disconnected,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode_frame, encode_frame, uuid_to_bytes, v3_envelope::RequestIdGenerator};
    use bytes::BytesMut;

    fn rt() -> uuid::Uuid {
        uuid::Uuid::new_v4()
    }

    fn client() -> uuid::Uuid {
        uuid::Uuid::new_v4()
    }

    // ── is_supported ──

    #[test]
    fn supported_when_capability_present() {
        let caps = vec![
            v3::Capability::CoreRuntimeLifecycle as i32,
            v3::Capability::OptRuntimeTakeover as i32,
        ];
        assert!(is_supported(&caps));
    }

    #[test]
    fn not_supported_when_capability_absent() {
        let caps = vec![
            v3::Capability::CoreRuntimeLifecycle as i32,
            v3::Capability::OptDiagnostics as i32,
        ];
        assert!(!is_supported(&caps));
    }

    #[test]
    fn not_supported_with_empty_caps() {
        assert!(!is_supported(&[]));
    }

    // ── build_takeover_runtime ──

    #[test]
    fn takeover_runtime_populates_runtime_id() {
        let r = rt();
        let req = build_takeover_runtime(r);
        assert_eq!(req.runtime_id, uuid_to_bytes(r));
    }

    #[test]
    fn takeover_runtime_wire_roundtrip() {
        let req = build_takeover_runtime(rt());
        let mut buf = BytesMut::new();
        encode_frame(&req, &mut buf).unwrap();
        let decoded: v3::TakeoverRuntime = decode_frame(&mut buf).unwrap();
        assert_eq!(req, decoded);
    }

    // ── build_takeover_runtime_envelope ──

    #[test]
    fn takeover_envelope_has_nonzero_request_id() {
        let id_gen = RequestIdGenerator::new();
        let req = build_takeover_runtime(rt());
        let env = build_takeover_runtime_envelope(&id_gen, req);
        assert_ne!(env.request_id, 0);
    }

    #[test]
    fn takeover_envelope_contains_correct_command() {
        let id_gen = RequestIdGenerator::new();
        let r = rt();
        let req = build_takeover_runtime(r);
        let env = build_takeover_runtime_envelope(&id_gen, req);
        match env.command {
            Some(v3::client_envelope::Command::TakeoverRuntime(ref tr)) => {
                assert_eq!(tr.runtime_id, uuid_to_bytes(r));
            }
            _ => panic!("expected TakeoverRuntime command"),
        }
    }

    #[test]
    fn takeover_envelope_wire_roundtrip() {
        let id_gen = RequestIdGenerator::new();
        let req = build_takeover_runtime(rt());
        let env = build_takeover_runtime_envelope(&id_gen, req);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ClientEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }

    // ── build_takeover_completed ──

    #[test]
    fn takeover_completed_populates_fields() {
        let r = rt();
        let completed = build_takeover_completed(r, 42);
        assert_eq!(completed.runtime_id, uuid_to_bytes(r));
        assert_eq!(completed.runtime_revision, 42);
    }

    #[test]
    fn takeover_completed_wire_roundtrip() {
        let completed = build_takeover_completed(rt(), 99);
        let mut buf = BytesMut::new();
        encode_frame(&completed, &mut buf).unwrap();
        let decoded: v3::TakeoverCompleted = decode_frame(&mut buf).unwrap();
        assert_eq!(completed, decoded);
    }

    // ── build_takeover_completed_response ──

    #[test]
    fn takeover_completed_response_echoes_request_id() {
        let completed = build_takeover_completed(rt(), 10);
        let env = build_takeover_completed_response(7, completed);
        assert_eq!(env.request_id, 7);
    }

    #[test]
    fn takeover_completed_response_contains_correct_payload() {
        let r = rt();
        let completed = build_takeover_completed(r, 20);
        let env = build_takeover_completed_response(5, completed);
        match env.payload {
            Some(v3::server_envelope::Payload::TakeoverCompleted(ref tc)) => {
                assert_eq!(tc.runtime_id, uuid_to_bytes(r));
                assert_eq!(tc.runtime_revision, 20);
            }
            _ => panic!("expected TakeoverCompleted payload"),
        }
    }

    #[test]
    fn takeover_completed_response_is_not_push_event() {
        let completed = build_takeover_completed(rt(), 1);
        let env = build_takeover_completed_response(42, completed);
        assert!(!crate::v3_envelope::is_push_event(&env));
    }

    #[test]
    fn takeover_completed_response_wire_roundtrip() {
        let completed = build_takeover_completed(rt(), 55);
        let env = build_takeover_completed_response(33, completed);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }

    // ── build_lease_lost ──

    #[test]
    fn lease_lost_populates_fields() {
        let r = rt();
        let new_owner = client();
        let lost = build_lease_lost(r, 30, new_owner);
        assert_eq!(lost.runtime_id, uuid_to_bytes(r));
        assert_eq!(lost.runtime_revision, 30);
        assert_eq!(lost.new_owner_id, uuid_to_bytes(new_owner));
    }

    #[test]
    fn lease_lost_wire_roundtrip() {
        let lost = build_lease_lost(rt(), 42, client());
        let mut buf = BytesMut::new();
        encode_frame(&lost, &mut buf).unwrap();
        let decoded: v3::LeaseLost = decode_frame(&mut buf).unwrap();
        assert_eq!(lost, decoded);
    }

    // ── build_lease_lost_envelope ──

    #[test]
    fn lease_lost_envelope_is_push_event() {
        let lost = build_lease_lost(rt(), 10, client());
        let env = build_lease_lost_envelope(lost);
        assert_eq!(env.request_id, 0);
        assert!(crate::v3_envelope::is_push_event(&env));
    }

    #[test]
    fn lease_lost_envelope_contains_correct_payload() {
        let r = rt();
        let new_owner = client();
        let lost = build_lease_lost(r, 15, new_owner);
        let env = build_lease_lost_envelope(lost);
        match env.payload {
            Some(v3::server_envelope::Payload::LeaseLost(ref ll)) => {
                assert_eq!(ll.runtime_id, uuid_to_bytes(r));
                assert_eq!(ll.runtime_revision, 15);
                assert_eq!(ll.new_owner_id, uuid_to_bytes(new_owner));
            }
            _ => panic!("expected LeaseLost payload"),
        }
    }

    #[test]
    fn lease_lost_envelope_wire_roundtrip() {
        let lost = build_lease_lost(rt(), 50, client());
        let env = build_lease_lost_envelope(lost);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }

    // ── build_owner_disconnected ──

    #[test]
    fn owner_disconnected_populates_fields() {
        let r = rt();
        let disconnected = build_owner_disconnected(r, 25);
        assert_eq!(disconnected.runtime_id, uuid_to_bytes(r));
        assert_eq!(disconnected.runtime_revision, 25);
    }

    #[test]
    fn owner_disconnected_wire_roundtrip() {
        let disconnected = build_owner_disconnected(rt(), 77);
        let mut buf = BytesMut::new();
        encode_frame(&disconnected, &mut buf).unwrap();
        let decoded: v3::OwnerDisconnected = decode_frame(&mut buf).unwrap();
        assert_eq!(disconnected, decoded);
    }

    // ── build_owner_disconnected_envelope ──

    #[test]
    fn owner_disconnected_envelope_is_push_event() {
        let disconnected = build_owner_disconnected(rt(), 10);
        let env = build_owner_disconnected_envelope(disconnected);
        assert_eq!(env.request_id, 0);
        assert!(crate::v3_envelope::is_push_event(&env));
    }

    #[test]
    fn owner_disconnected_envelope_contains_correct_payload() {
        let r = rt();
        let disconnected = build_owner_disconnected(r, 33);
        let env = build_owner_disconnected_envelope(disconnected);
        match env.payload {
            Some(v3::server_envelope::Payload::OwnerDisconnected(ref od)) => {
                assert_eq!(od.runtime_id, uuid_to_bytes(r));
                assert_eq!(od.runtime_revision, 33);
            }
            _ => panic!("expected OwnerDisconnected payload"),
        }
    }

    #[test]
    fn owner_disconnected_envelope_wire_roundtrip() {
        let disconnected = build_owner_disconnected(rt(), 88);
        let env = build_owner_disconnected_envelope(disconnected);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }

    // ── Integration: unsupported capability error ──

    #[test]
    fn unsupported_capability_error_for_takeover() {
        let err = crate::v3_error::build_error(
            v3::ErrorKind::UnsupportedCapability,
            "OPT_RUNTIME_TAKEOVER not negotiated",
            "TakeoverRuntime",
        );
        let env = crate::v3_error::build_error_response(42, err);
        assert_eq!(env.request_id, 42);
        match env.payload {
            Some(v3::server_envelope::Payload::Error(ref e)) => {
                assert_eq!(e.kind, v3::ErrorKind::UnsupportedCapability as i32);
                assert_eq!(e.operation, "TakeoverRuntime");
            }
            _ => panic!("expected Error payload"),
        }
    }

    // ── Integration: ownership conflict without takeover capability ──

    #[test]
    fn ownership_conflict_without_takeover() {
        let caps = vec![v3::Capability::CoreRuntimeLifecycle as i32];
        assert!(!is_supported(&caps));

        let err = crate::v3_error::build_error(
            v3::ErrorKind::OwnershipConflict,
            "runtime owned by another client; takeover not available",
            "AttachRuntime",
        );
        let env = crate::v3_error::build_error_response(10, err);
        match env.payload {
            Some(v3::server_envelope::Payload::Error(ref e)) => {
                assert_eq!(e.kind, v3::ErrorKind::OwnershipConflict as i32);
                assert!(!e.retryable);
            }
            _ => panic!("expected Error payload"),
        }
    }

    // ── Integration: takeover_required error ──

    #[test]
    fn takeover_required_error() {
        let err = crate::v3_error::build_error(
            v3::ErrorKind::TakeoverRequired,
            "runtime has an active writer; use TakeoverRuntime to claim it",
            "AttachRuntime",
        );
        assert!(!err.retryable);
        assert!(err.user_action_required);
    }

    // ── Integration: full takeover flow ──

    #[test]
    fn full_takeover_flow() {
        let id_gen = RequestIdGenerator::new();
        let r = rt();
        let old_owner = client();
        let new_owner = client();

        // 1. Client sends TakeoverRuntime request
        let req = build_takeover_runtime(r);
        let req_env = build_takeover_runtime_envelope(&id_gen, req);
        let saved_request_id = req_env.request_id;
        assert_ne!(saved_request_id, 0);

        // 2. Server sends LeaseLost to the previous writer
        let lost = build_lease_lost(r, 50, new_owner);
        let lost_env = build_lease_lost_envelope(lost);
        assert!(crate::v3_envelope::is_push_event(&lost_env));
        match lost_env.payload {
            Some(v3::server_envelope::Payload::LeaseLost(ref ll)) => {
                assert_eq!(ll.new_owner_id, uuid_to_bytes(new_owner));
            }
            _ => panic!("expected LeaseLost"),
        }

        // 3. Server responds to the requesting client with TakeoverCompleted
        let completed = build_takeover_completed(r, 51);
        let completed_env = build_takeover_completed_response(saved_request_id, completed);
        assert_eq!(completed_env.request_id, saved_request_id);
        assert!(!crate::v3_envelope::is_push_event(&completed_env));

        // Verify the old owner ID is not in the completed response (it's only in LeaseLost)
        match completed_env.payload {
            Some(v3::server_envelope::Payload::TakeoverCompleted(ref tc)) => {
                assert_eq!(tc.runtime_id, uuid_to_bytes(r));
                assert_eq!(tc.runtime_revision, 51);
            }
            _ => panic!("expected TakeoverCompleted"),
        }

        // 4. Verify old_owner and new_owner are distinct
        assert_ne!(uuid_to_bytes(old_owner), uuid_to_bytes(new_owner));
    }

    // ── Integration: owner disconnect notifies readers ──

    #[test]
    fn owner_disconnect_notifies_readers() {
        let r = rt();

        // Writer disconnects; server sends OwnerDisconnected to all readers
        let disconnected = build_owner_disconnected(r, 60);
        let env = build_owner_disconnected_envelope(disconnected);
        assert!(crate::v3_envelope::is_push_event(&env));
        match env.payload {
            Some(v3::server_envelope::Payload::OwnerDisconnected(ref od)) => {
                assert_eq!(od.runtime_id, uuid_to_bytes(r));
                assert_eq!(od.runtime_revision, 60);
            }
            _ => panic!("expected OwnerDisconnected"),
        }
    }

    // ── Integration: attach blocked then takeover ──

    #[test]
    fn attach_blocked_then_takeover() {
        let id_gen = RequestIdGenerator::new();
        let r = rt();

        // 1. Client tries read-write attach, gets AttachBlocked
        let blocked = v3::AttachBlocked {
            runtime_id: uuid_to_bytes(r),
            current_client_role: v3::RuntimeClientRole::Unattached as i32,
            attached_client_count: 1,
            read_only_client_count: 0,
        };
        let blocked_env = crate::v3_envelope::build_response_envelope(
            1,
            v3::server_envelope::Payload::AttachBlocked(blocked),
        );
        match blocked_env.payload {
            Some(v3::server_envelope::Payload::AttachBlocked(ref ab)) => {
                assert_eq!(ab.attached_client_count, 1);
            }
            _ => panic!("expected AttachBlocked"),
        }

        // 2. Client has OPT_RUNTIME_TAKEOVER, so it sends TakeoverRuntime
        let caps = vec![
            v3::Capability::CoreRuntimeLifecycle as i32,
            v3::Capability::OptRuntimeTakeover as i32,
        ];
        assert!(is_supported(&caps));

        let req = build_takeover_runtime(r);
        let env = build_takeover_runtime_envelope(&id_gen, req);
        assert_ne!(env.request_id, 0);

        // 3. Server responds with TakeoverCompleted
        let completed = build_takeover_completed(r, 70);
        let resp = build_takeover_completed_response(env.request_id, completed);
        assert_eq!(resp.request_id, env.request_id);
    }
}

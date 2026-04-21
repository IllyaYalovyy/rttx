//! V3 envelope: request/response correlation and command classification.
//!
//! Implements RFC-021 Section 2 (Command/Event Envelopes).
//!
//! - Fire-and-forget commands (`TerminalInput`, `ResizePane`, `SetPaneTitle`,
//!   `Shutdown`) carry `request_id = 0` and receive no response.
//! - Request/response commands carry a non-zero `request_id` assigned by the
//!   client. The server echoes the `request_id` in the response.
//! - Push events from the server carry `request_id = 0`.

use crate::v3;
use std::sync::atomic::{AtomicU64, Ordering};

/// Generates unique non-zero request IDs for a single client connection.
///
/// IDs are monotonically increasing and wrap at `u64::MAX` back to 1.
/// Thread-safe via atomic operations.
pub struct RequestIdGenerator {
    next: AtomicU64,
}

impl RequestIdGenerator {
    /// Create a new generator starting at 1.
    #[must_use]
    pub fn new() -> Self {
        Self { next: AtomicU64::new(1) }
    }

    /// Return the next unique non-zero request ID.
    pub fn next_id(&self) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        // Wrap past zero — u64::MAX + 1 wraps to 0, so skip it.
        if id == 0 { self.next.fetch_add(1, Ordering::Relaxed) } else { id }
    }
}

impl Default for RequestIdGenerator {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns `true` if the command variant is fire-and-forget (no response expected).
#[must_use]
pub fn is_fire_and_forget(command: &v3::client_envelope::Command) -> bool {
    matches!(
        command,
        v3::client_envelope::Command::TerminalInput(_)
            | v3::client_envelope::Command::ResizePane(_)
            | v3::client_envelope::Command::SetPaneTitle(_)
            | v3::client_envelope::Command::SetPaneNoPersist(_)
            | v3::client_envelope::Command::Shutdown(_)
    )
}

/// Build a `ClientEnvelope` with the correct `request_id`.
///
/// Fire-and-forget commands get `request_id = 0`.
/// Request/response commands get a non-zero ID from the generator.
#[must_use]
pub fn build_client_envelope(
    id_gen: &RequestIdGenerator,
    command: v3::client_envelope::Command,
) -> v3::ClientEnvelope {
    let request_id = if is_fire_and_forget(&command) { 0 } else { id_gen.next_id() };
    v3::ClientEnvelope { request_id, command: Some(command) }
}

/// Build a `ServerEnvelope` response echoing the client's `request_id`.
#[must_use]
pub fn build_response_envelope(
    request_id: u64,
    payload: v3::server_envelope::Payload,
) -> v3::ServerEnvelope {
    v3::ServerEnvelope { request_id, payload: Some(payload) }
}

/// Build a `ServerEnvelope` push event (`request_id = 0`).
#[must_use]
pub fn build_push_envelope(payload: v3::server_envelope::Payload) -> v3::ServerEnvelope {
    v3::ServerEnvelope { request_id: 0, payload: Some(payload) }
}

/// Returns `true` if the server envelope is a push event (not a response).
#[must_use]
pub fn is_push_event(envelope: &v3::ServerEnvelope) -> bool {
    envelope.request_id == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uuid_to_bytes;

    fn rid() -> Vec<u8> {
        uuid_to_bytes(uuid::Uuid::new_v4())
    }

    fn pid() -> Vec<u8> {
        uuid_to_bytes(uuid::Uuid::new_v4())
    }

    // ── RequestIdGenerator ──

    #[test]
    fn generator_starts_at_one() {
        let id_gen = RequestIdGenerator::new();
        assert_eq!(id_gen.next_id(), 1);
    }

    #[test]
    fn generator_increments() {
        let id_gen = RequestIdGenerator::new();
        assert_eq!(id_gen.next_id(), 1);
        assert_eq!(id_gen.next_id(), 2);
        assert_eq!(id_gen.next_id(), 3);
    }

    #[test]
    fn generator_never_returns_zero() {
        // Force the counter near the wrap point.
        let id_gen = RequestIdGenerator { next: AtomicU64::new(u64::MAX) };
        let id1 = id_gen.next_id();
        assert_eq!(id1, u64::MAX);
        // Next call wraps past 0.
        let id2 = id_gen.next_id();
        assert_ne!(id2, 0, "generator must never return zero");
    }

    #[test]
    fn generator_default_matches_new() {
        let id_gen = RequestIdGenerator::default();
        assert_eq!(id_gen.next_id(), 1);
    }

    // ── Fire-and-forget classification ──

    #[test]
    fn terminal_input_is_fire_and_forget() {
        let cmd = v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
            runtime_id: rid(),
            pane_id: pid(),
            kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                data: bytes::Bytes::from_static(b"x"),
            })),
        });
        assert!(is_fire_and_forget(&cmd));
    }

    #[test]
    fn resize_pane_is_fire_and_forget() {
        let cmd = v3::client_envelope::Command::ResizePane(v3::ResizePane {
            runtime_id: rid(),
            pane_id: pid(),
            cols: 80,
            rows: 24,
        });
        assert!(is_fire_and_forget(&cmd));
    }

    #[test]
    fn set_pane_title_is_fire_and_forget() {
        let cmd = v3::client_envelope::Command::SetPaneTitle(v3::SetPaneTitle {
            runtime_id: rid(),
            pane_id: pid(),
            title: "t".into(),
        });
        assert!(is_fire_and_forget(&cmd));
    }

    #[test]
    fn shutdown_is_fire_and_forget() {
        let cmd = v3::client_envelope::Command::Shutdown(v3::Shutdown {});
        assert!(is_fire_and_forget(&cmd));
    }

    #[test]
    fn ping_is_not_fire_and_forget() {
        let cmd = v3::client_envelope::Command::Ping(v3::Ping { nonce: 1 });
        assert!(!is_fire_and_forget(&cmd));
    }

    #[test]
    fn create_runtime_is_not_fire_and_forget() {
        let cmd = v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
            name: "test".into(),
            policy: v3::RuntimePolicy::Ephemeral as i32,
        });
        assert!(!is_fire_and_forget(&cmd));
    }

    #[test]
    fn attach_runtime_is_not_fire_and_forget() {
        let cmd = v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
            runtime_id: rid(),
            attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
        });
        assert!(!is_fire_and_forget(&cmd));
    }

    #[test]
    fn detach_runtime_is_not_fire_and_forget() {
        let cmd =
            v3::client_envelope::Command::DetachRuntime(v3::DetachRuntime { runtime_id: rid() });
        assert!(!is_fire_and_forget(&cmd));
    }

    #[test]
    fn terminate_runtime_is_not_fire_and_forget() {
        let cmd = v3::client_envelope::Command::TerminateRuntime(v3::TerminateRuntime {
            runtime_id: rid(),
        });
        assert!(!is_fire_and_forget(&cmd));
    }

    #[test]
    fn rename_runtime_is_not_fire_and_forget() {
        let cmd = v3::client_envelope::Command::RenameRuntime(v3::RenameRuntime {
            runtime_id: rid(),
            name: "new".into(),
        });
        assert!(!is_fire_and_forget(&cmd));
    }

    #[test]
    fn list_runtimes_is_not_fire_and_forget() {
        let cmd = v3::client_envelope::Command::ListRuntimes(v3::ListRuntimes {});
        assert!(!is_fire_and_forget(&cmd));
    }

    #[test]
    fn create_pane_is_not_fire_and_forget() {
        let cmd = v3::client_envelope::Command::CreatePane(v3::CreatePane {
            runtime_id: rid(),
            cwd: None,
            dark_background: None,
            cols: 80,
            rows: 24,
            no_persist: None,
        });
        assert!(!is_fire_and_forget(&cmd));
    }

    #[test]
    fn close_pane_is_not_fire_and_forget() {
        let cmd = v3::client_envelope::Command::ClosePane(v3::ClosePane {
            runtime_id: rid(),
            pane_id: pid(),
        });
        assert!(!is_fire_and_forget(&cmd));
    }

    #[test]
    fn resync_runtime_is_not_fire_and_forget() {
        let cmd =
            v3::client_envelope::Command::ResyncRuntime(v3::ResyncRuntime { runtime_id: rid() });
        assert!(!is_fire_and_forget(&cmd));
    }

    #[test]
    fn get_scrollback_is_not_fire_and_forget() {
        let cmd = v3::client_envelope::Command::GetScrollback(v3::GetScrollback {
            runtime_id: rid(),
            pane_id: pid(),
            offset: 0,
            limit: 65536,
        });
        assert!(!is_fire_and_forget(&cmd));
    }

    #[test]
    fn get_diagnostics_is_not_fire_and_forget() {
        let cmd = v3::client_envelope::Command::GetDiagnostics(v3::GetDiagnostics {});
        assert!(!is_fire_and_forget(&cmd));
    }

    #[test]
    fn takeover_runtime_is_not_fire_and_forget() {
        let cmd = v3::client_envelope::Command::TakeoverRuntime(v3::TakeoverRuntime {
            runtime_id: rid(),
        });
        assert!(!is_fire_and_forget(&cmd));
    }

    // ── build_client_envelope ──

    #[test]
    fn fire_and_forget_envelope_has_zero_request_id() {
        let id_gen = RequestIdGenerator::new();
        let cmd = v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
            runtime_id: rid(),
            pane_id: pid(),
            kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                data: bytes::Bytes::from_static(b"a"),
            })),
        });
        let env = build_client_envelope(&id_gen, cmd);
        assert_eq!(env.request_id, 0);
    }

    #[test]
    fn request_response_envelope_has_nonzero_request_id() {
        let id_gen = RequestIdGenerator::new();
        let cmd = v3::client_envelope::Command::Ping(v3::Ping { nonce: 42 });
        let env = build_client_envelope(&id_gen, cmd);
        assert_ne!(env.request_id, 0);
    }

    #[test]
    fn fire_and_forget_does_not_consume_request_ids() {
        let id_gen = RequestIdGenerator::new();
        // Send several fire-and-forget commands.
        for _ in 0..5 {
            let cmd = v3::client_envelope::Command::Shutdown(v3::Shutdown {});
            let env = build_client_envelope(&id_gen, cmd);
            assert_eq!(env.request_id, 0);
        }
        // The first request/response command should still get ID 1.
        let cmd = v3::client_envelope::Command::Ping(v3::Ping { nonce: 1 });
        let env = build_client_envelope(&id_gen, cmd);
        assert_eq!(env.request_id, 1);
    }

    #[test]
    fn sequential_request_ids_are_unique() {
        let id_gen = RequestIdGenerator::new();
        let mut ids = Vec::new();
        for _ in 0..100 {
            let cmd = v3::client_envelope::Command::Ping(v3::Ping { nonce: 0 });
            let env = build_client_envelope(&id_gen, cmd);
            assert!(!ids.contains(&env.request_id), "duplicate request_id");
            ids.push(env.request_id);
        }
    }

    // ── build_response_envelope / build_push_envelope ──

    #[test]
    fn response_envelope_echoes_request_id() {
        let payload = v3::server_envelope::Payload::Pong(v3::Pong { nonce: 42 });
        let env = build_response_envelope(7, payload);
        assert_eq!(env.request_id, 7);
    }

    #[test]
    fn push_envelope_has_zero_request_id() {
        let payload = v3::server_envelope::Payload::OutputDelta(v3::OutputDelta {
            runtime_id: rid(),
            pane_id: pid(),
            data: bytes::Bytes::from_static(b"out"),
            pane_output_seq: 1,
        });
        let env = build_push_envelope(payload);
        assert_eq!(env.request_id, 0);
    }

    // ── is_push_event ──

    #[test]
    fn push_event_detected() {
        let env = v3::ServerEnvelope {
            request_id: 0,
            payload: Some(v3::server_envelope::Payload::PaneExited(v3::PaneExited {
                runtime_id: rid(),
                pane_id: pid(),
                status: 0,
                runtime_revision: 1,
            })),
        };
        assert!(is_push_event(&env));
    }

    #[test]
    fn response_not_detected_as_push() {
        let env = v3::ServerEnvelope {
            request_id: 5,
            payload: Some(v3::server_envelope::Payload::Pong(v3::Pong { nonce: 5 })),
        };
        assert!(!is_push_event(&env));
    }

    // ── Wire roundtrip through encode/decode ──

    #[test]
    fn client_envelope_roundtrip_through_frame() {
        let id_gen = RequestIdGenerator::new();
        let cmd = v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
            name: "test".into(),
            policy: v3::RuntimePolicy::Persistent as i32,
        });
        let env = build_client_envelope(&id_gen, cmd);
        assert_ne!(env.request_id, 0);

        let mut buf = bytes::BytesMut::new();
        crate::encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ClientEnvelope = crate::decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }

    #[test]
    fn server_response_roundtrip_through_frame() {
        let payload = v3::server_envelope::Payload::RuntimeCreated(v3::RuntimeCreated {
            runtime_id: rid(),
            runtime_revision: 1,
        });
        let env = build_response_envelope(42, payload);

        let mut buf = bytes::BytesMut::new();
        crate::encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ServerEnvelope = crate::decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }

    #[test]
    fn server_push_roundtrip_through_frame() {
        let payload =
            v3::server_envelope::Payload::Bell(v3::Bell { runtime_id: rid(), pane_id: pid() });
        let env = build_push_envelope(payload);

        let mut buf = bytes::BytesMut::new();
        crate::encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ServerEnvelope = crate::decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
        assert!(is_push_event(&decoded));
    }
}

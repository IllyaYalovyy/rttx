//! V3 error model: typed `ProtocolError` builders and `ErrorKind` classification.
//!
//! Implements RFC-021 Section 11 (Error Model).
//!
//! `ProtocolError` is the only message type that may appear both inside a
//! `ServerEnvelope` and as a bare handshake-phase message. Builders set
//! default retryability and `user_action_required` based on `ErrorKind`,
//! which the caller can override.
//!
//! The client maps `ErrorKind` to `ConnectionProblem` and UI policy via
//! [`classify_error_kind`] — no string matching required.

use crate::v3;

/// Classification of an `ErrorKind` for client-side connection state machines.
///
/// This is a protocol-level classification that the client maps to its own
/// `ConnectionProblem` enum. Keeping it in `rttx-proto` avoids duplicating
/// the mapping logic and ensures the protocol crate is the single source of
/// truth for error semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClassification {
    /// Version or capability mismatch — not retryable, user must update.
    IncompatibleVersion,
    /// The target resource (runtime or pane) does not exist.
    ResourceNotFound,
    /// Another client owns the runtime — user must decide (takeover or wait).
    OwnershipConflict,
    /// The server's push channel overflowed — retryable via resync.
    StreamOverflow,
    /// A transient server-side error — retryable automatically.
    TransientError,
    /// The client sent an invalid request — not retryable without fixing input.
    InvalidRequest,
    /// Unknown or unrecognized error kind — fallback.
    Unknown,
}

/// Classify an `ErrorKind` into a connection-level category.
///
/// Clients use this to map typed errors to UI policy without string matching.
/// Unknown enum values (from a newer server) are classified as `Unknown`.
#[must_use]
pub fn classify_error_kind(kind: v3::ErrorKind) -> ErrorClassification {
    match kind {
        v3::ErrorKind::ProtocolMismatch | v3::ErrorKind::UnsupportedCapability => {
            ErrorClassification::IncompatibleVersion
        }
        v3::ErrorKind::RuntimeNotFound | v3::ErrorKind::PaneNotFound => {
            ErrorClassification::ResourceNotFound
        }
        v3::ErrorKind::OwnershipConflict | v3::ErrorKind::TakeoverRequired => {
            ErrorClassification::OwnershipConflict
        }
        v3::ErrorKind::StreamOverflow => ErrorClassification::StreamOverflow,
        v3::ErrorKind::Internal => ErrorClassification::TransientError,
        v3::ErrorKind::InvalidArgument => ErrorClassification::InvalidRequest,
        v3::ErrorKind::Unspecified => ErrorClassification::Unknown,
    }
}

/// Returns the default retryability for an `ErrorKind`.
///
/// - `StreamOverflow` and `Internal` are retryable (transient).
/// - All others are not retryable by default.
#[must_use]
pub fn is_default_retryable(kind: v3::ErrorKind) -> bool {
    matches!(kind, v3::ErrorKind::StreamOverflow | v3::ErrorKind::Internal)
}

/// Returns whether the error kind typically requires user action.
#[must_use]
pub fn is_default_user_action_required(kind: v3::ErrorKind) -> bool {
    matches!(
        kind,
        v3::ErrorKind::ProtocolMismatch
            | v3::ErrorKind::UnsupportedCapability
            | v3::ErrorKind::OwnershipConflict
            | v3::ErrorKind::TakeoverRequired
    )
}

/// Build a `ProtocolError` with defaults derived from the `ErrorKind`.
#[must_use]
pub fn build_error(kind: v3::ErrorKind, message: &str, operation: &str) -> v3::ProtocolError {
    v3::ProtocolError {
        kind: kind as i32,
        message: message.into(),
        operation: operation.into(),
        retryable: is_default_retryable(kind),
        user_action_required: is_default_user_action_required(kind),
        retry_after_seconds: 0,
    }
}

/// Build a `ProtocolError` with an explicit retry hint.
#[must_use]
pub fn build_retryable_error(
    kind: v3::ErrorKind,
    message: &str,
    operation: &str,
    retry_after_seconds: u32,
) -> v3::ProtocolError {
    v3::ProtocolError {
        kind: kind as i32,
        message: message.into(),
        operation: operation.into(),
        retryable: true,
        user_action_required: false,
        retry_after_seconds,
    }
}

/// Build a `ServerEnvelope` error response echoing the client's `request_id`.
#[must_use]
pub fn build_error_response(request_id: u64, error: v3::ProtocolError) -> v3::ServerEnvelope {
    crate::v3_envelope::build_response_envelope(
        request_id,
        v3::server_envelope::Payload::Error(error),
    )
}

/// Extract the typed `ErrorKind` from a `ProtocolError`.
///
/// Unknown `i32` values (from a newer server) return `ErrorKind::Unspecified`.
#[must_use]
pub fn error_kind(error: &v3::ProtocolError) -> v3::ErrorKind {
    v3::ErrorKind::try_from(error.kind).unwrap_or(v3::ErrorKind::Unspecified)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode_frame, encode_frame};
    use bytes::BytesMut;

    // ── classify_error_kind ──

    #[test]
    fn classify_protocol_mismatch() {
        assert_eq!(
            classify_error_kind(v3::ErrorKind::ProtocolMismatch),
            ErrorClassification::IncompatibleVersion
        );
    }

    #[test]
    fn classify_unsupported_capability() {
        assert_eq!(
            classify_error_kind(v3::ErrorKind::UnsupportedCapability),
            ErrorClassification::IncompatibleVersion
        );
    }

    #[test]
    fn classify_invalid_argument() {
        assert_eq!(
            classify_error_kind(v3::ErrorKind::InvalidArgument),
            ErrorClassification::InvalidRequest
        );
    }

    #[test]
    fn classify_runtime_not_found() {
        assert_eq!(
            classify_error_kind(v3::ErrorKind::RuntimeNotFound),
            ErrorClassification::ResourceNotFound
        );
    }

    #[test]
    fn classify_pane_not_found() {
        assert_eq!(
            classify_error_kind(v3::ErrorKind::PaneNotFound),
            ErrorClassification::ResourceNotFound
        );
    }

    #[test]
    fn classify_ownership_conflict() {
        assert_eq!(
            classify_error_kind(v3::ErrorKind::OwnershipConflict),
            ErrorClassification::OwnershipConflict
        );
    }

    #[test]
    fn classify_takeover_required() {
        assert_eq!(
            classify_error_kind(v3::ErrorKind::TakeoverRequired),
            ErrorClassification::OwnershipConflict
        );
    }

    #[test]
    fn classify_stream_overflow() {
        assert_eq!(
            classify_error_kind(v3::ErrorKind::StreamOverflow),
            ErrorClassification::StreamOverflow
        );
    }

    #[test]
    fn classify_internal() {
        assert_eq!(
            classify_error_kind(v3::ErrorKind::Internal),
            ErrorClassification::TransientError
        );
    }

    #[test]
    fn classify_unspecified() {
        assert_eq!(
            classify_error_kind(v3::ErrorKind::Unspecified),
            ErrorClassification::Unknown
        );
    }

    // ── is_default_retryable ──

    #[test]
    fn stream_overflow_is_retryable() {
        assert!(is_default_retryable(v3::ErrorKind::StreamOverflow));
    }

    #[test]
    fn internal_is_retryable() {
        assert!(is_default_retryable(v3::ErrorKind::Internal));
    }

    #[test]
    fn protocol_mismatch_not_retryable() {
        assert!(!is_default_retryable(v3::ErrorKind::ProtocolMismatch));
    }

    #[test]
    fn runtime_not_found_not_retryable() {
        assert!(!is_default_retryable(v3::ErrorKind::RuntimeNotFound));
    }

    #[test]
    fn ownership_conflict_not_retryable() {
        assert!(!is_default_retryable(v3::ErrorKind::OwnershipConflict));
    }

    #[test]
    fn invalid_argument_not_retryable() {
        assert!(!is_default_retryable(v3::ErrorKind::InvalidArgument));
    }

    // ── is_default_user_action_required ──

    #[test]
    fn protocol_mismatch_requires_user_action() {
        assert!(is_default_user_action_required(v3::ErrorKind::ProtocolMismatch));
    }

    #[test]
    fn unsupported_capability_requires_user_action() {
        assert!(is_default_user_action_required(v3::ErrorKind::UnsupportedCapability));
    }

    #[test]
    fn ownership_conflict_requires_user_action() {
        assert!(is_default_user_action_required(v3::ErrorKind::OwnershipConflict));
    }

    #[test]
    fn takeover_required_requires_user_action() {
        assert!(is_default_user_action_required(v3::ErrorKind::TakeoverRequired));
    }

    #[test]
    fn internal_does_not_require_user_action() {
        assert!(!is_default_user_action_required(v3::ErrorKind::Internal));
    }

    #[test]
    fn stream_overflow_does_not_require_user_action() {
        assert!(!is_default_user_action_required(v3::ErrorKind::StreamOverflow));
    }

    #[test]
    fn runtime_not_found_does_not_require_user_action() {
        assert!(!is_default_user_action_required(v3::ErrorKind::RuntimeNotFound));
    }

    // ── build_error ──

    #[test]
    fn build_error_sets_kind_and_message() {
        let err = build_error(v3::ErrorKind::RuntimeNotFound, "runtime abc not found", "AttachRuntime");
        assert_eq!(err.kind, v3::ErrorKind::RuntimeNotFound as i32);
        assert_eq!(err.message, "runtime abc not found");
        assert_eq!(err.operation, "AttachRuntime");
    }

    #[test]
    fn build_error_applies_default_retryability() {
        let err = build_error(v3::ErrorKind::Internal, "oops", "CreatePane");
        assert!(err.retryable);
        assert!(!err.user_action_required);

        let err = build_error(v3::ErrorKind::ProtocolMismatch, "mismatch", "Handshake");
        assert!(!err.retryable);
        assert!(err.user_action_required);
    }

    #[test]
    fn build_error_default_retry_after_is_zero() {
        let err = build_error(v3::ErrorKind::StreamOverflow, "overflow", "OutputDelta");
        assert_eq!(err.retry_after_seconds, 0);
    }

    // ── build_retryable_error ──

    #[test]
    fn build_retryable_error_sets_retry_hint() {
        let err = build_retryable_error(v3::ErrorKind::Internal, "busy", "CreateRuntime", 5);
        assert!(err.retryable);
        assert!(!err.user_action_required);
        assert_eq!(err.retry_after_seconds, 5);
    }

    // ── build_error_response ──

    #[test]
    fn error_response_echoes_request_id() {
        let err = build_error(v3::ErrorKind::PaneNotFound, "not found", "ClosePane");
        let env = build_error_response(42, err.clone());
        assert_eq!(env.request_id, 42);
        match env.payload {
            Some(v3::server_envelope::Payload::Error(ref e)) => assert_eq!(e, &err),
            _ => panic!("expected Error payload"),
        }
    }

    #[test]
    fn error_response_wire_roundtrip() {
        let err = build_error(v3::ErrorKind::OwnershipConflict, "busy", "AttachRuntime");
        let env = build_error_response(7, err);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }

    // ── error_kind extraction ──

    #[test]
    fn error_kind_extracts_known_kind() {
        let err = build_error(v3::ErrorKind::TakeoverRequired, "takeover", "AttachRuntime");
        assert_eq!(error_kind(&err), v3::ErrorKind::TakeoverRequired);
    }

    #[test]
    fn error_kind_unknown_value_returns_unspecified() {
        let err = v3::ProtocolError {
            kind: 999,
            message: "future error".into(),
            operation: "Unknown".into(),
            retryable: false,
            user_action_required: false,
            retry_after_seconds: 0,
        };
        assert_eq!(error_kind(&err), v3::ErrorKind::Unspecified);
    }

    // ── Bare handshake-phase ProtocolError ──

    #[test]
    fn bare_protocol_error_roundtrip() {
        let err = build_error(
            v3::ErrorKind::ProtocolMismatch,
            "no common version: client v4–v4, server v3–v3",
            "Handshake",
        );
        let mut buf = BytesMut::new();
        encode_frame(&err, &mut buf).unwrap();
        let decoded: v3::ProtocolError = decode_frame(&mut buf).unwrap();
        assert_eq!(err, decoded);
    }

    #[test]
    fn bare_protocol_error_unsupported_capability() {
        let err = build_error(
            v3::ErrorKind::UnsupportedCapability,
            "server missing CORE_FOCUS_EVENTS",
            "Handshake",
        );
        let mut buf = BytesMut::new();
        encode_frame(&err, &mut buf).unwrap();
        let decoded: v3::ProtocolError = decode_frame(&mut buf).unwrap();
        assert_eq!(err, decoded);
        assert!(err.user_action_required);
        assert!(!err.retryable);
    }

    // ── All ErrorKind variants have consistent classification ──

    #[test]
    fn all_error_kinds_are_classified() {
        let kinds = [
            v3::ErrorKind::Unspecified,
            v3::ErrorKind::ProtocolMismatch,
            v3::ErrorKind::UnsupportedCapability,
            v3::ErrorKind::InvalidArgument,
            v3::ErrorKind::RuntimeNotFound,
            v3::ErrorKind::PaneNotFound,
            v3::ErrorKind::OwnershipConflict,
            v3::ErrorKind::TakeoverRequired,
            v3::ErrorKind::StreamOverflow,
            v3::ErrorKind::Internal,
        ];
        for kind in kinds {
            // Every kind must produce a valid classification (no panic).
            let _ = classify_error_kind(kind);
            let _ = is_default_retryable(kind);
            let _ = is_default_user_action_required(kind);
        }
    }

    // ── Retryable errors are not user-action-required and vice versa ──

    #[test]
    fn retryable_and_user_action_are_mutually_exclusive_by_default() {
        let kinds = [
            v3::ErrorKind::ProtocolMismatch,
            v3::ErrorKind::UnsupportedCapability,
            v3::ErrorKind::InvalidArgument,
            v3::ErrorKind::RuntimeNotFound,
            v3::ErrorKind::PaneNotFound,
            v3::ErrorKind::OwnershipConflict,
            v3::ErrorKind::TakeoverRequired,
            v3::ErrorKind::StreamOverflow,
            v3::ErrorKind::Internal,
        ];
        for kind in kinds {
            let retryable = is_default_retryable(kind);
            let user_action = is_default_user_action_required(kind);
            assert!(
                !(retryable && user_action),
                "{kind:?} should not be both retryable and user-action-required"
            );
        }
    }
}

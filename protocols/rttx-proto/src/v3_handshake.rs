//! V3 handshake: version negotiation, capability validation, message builders.
//!
//! Implements RFC-021 Section 1 (Version and Capability Negotiation).
//! The handshake is exchanged as bare length-prefixed protobuf messages
//! before the envelope protocol begins.

use crate::v3;

/// The protocol version implemented by this crate.
pub const V3_PROTOCOL_VERSION: u32 = 3;

/// All core capabilities that every v3 implementation must support.
pub const CORE_CAPABILITIES: &[v3::Capability] = &[
    v3::Capability::CoreWorkspaceLifecycle,
    v3::Capability::CorePaneLifecycle,
    v3::Capability::CoreTerminalIo,
    v3::Capability::CoreTerminalModes,
    v3::Capability::CorePasteIntent,
    v3::Capability::CoreFocusEvents,
];

/// Build a `ClientHello` message.
#[must_use]
pub fn build_client_hello(
    client_id: uuid::Uuid,
    client_name: &str,
    client_version: &str,
    capabilities: &[v3::Capability],
) -> v3::ClientHello {
    v3::ClientHello {
        min_protocol_version: V3_PROTOCOL_VERSION,
        max_protocol_version: V3_PROTOCOL_VERSION,
        client_id: crate::uuid_to_bytes(client_id),
        client_name: client_name.into(),
        client_version: client_version.into(),
        capabilities: capabilities.iter().map(|c| *c as i32).collect(),
    }
}

/// Build a `ServerHello` message.
#[must_use]
pub fn build_server_hello(
    server_id: uuid::Uuid,
    server_version: &str,
    negotiated_version: u32,
    capabilities: &[v3::Capability],
) -> v3::ServerHello {
    v3::ServerHello {
        negotiated_protocol_version: negotiated_version,
        server_id: crate::uuid_to_bytes(server_id),
        server_version: server_version.into(),
        capabilities: capabilities.iter().map(|c| *c as i32).collect(),
    }
}

/// Negotiate the highest mutually supported protocol version.
///
/// Returns the negotiated version, or a `ProtocolError` with kind
/// `PROTOCOL_MISMATCH` if no overlap exists.
pub fn negotiate_version(
    client_min: u32,
    client_max: u32,
    server_min: u32,
    server_max: u32,
) -> Result<u32, v3::ProtocolError> {
    let overlap_min = client_min.max(server_min);
    let overlap_max = client_max.min(server_max);
    if overlap_min <= overlap_max {
        Ok(overlap_max)
    } else {
        Err(v3::ProtocolError {
            kind: v3::ErrorKind::ProtocolMismatch as i32,
            message: format!(
                "no common protocol version: client supports v{client_min}–v{client_max}, \
                 server supports v{server_min}–v{server_max}"
            ),
            operation: "Handshake".into(),
            retryable: false,
            user_action_required: true,
            retry_after_seconds: 0,
        })
    }
}

/// Validate that the server advertises all core capabilities.
///
/// Returns `Ok(())` if all core capabilities are present, or `Err` with the
/// list of missing core capabilities.
pub fn validate_server_capabilities(server_caps: &[i32]) -> Result<(), Vec<v3::Capability>> {
    let missing: Vec<v3::Capability> = CORE_CAPABILITIES
        .iter()
        .filter(|core| !server_caps.contains(&(**core as i32)))
        .copied()
        .collect();
    if missing.is_empty() { Ok(()) } else { Err(missing) }
}

/// Compute the effective capability set (intersection of client and server).
#[must_use]
pub fn effective_capabilities(client_caps: &[i32], server_caps: &[i32]) -> Vec<i32> {
    client_caps.iter().filter(|c| server_caps.contains(c)).copied().collect()
}

/// Build a `ProtocolError` for missing core capabilities.
#[must_use]
pub fn missing_capabilities_error(missing: &[v3::Capability]) -> v3::ProtocolError {
    let names: Vec<&str> = missing
        .iter()
        .map(|c| match c {
            v3::Capability::CoreWorkspaceLifecycle => "CORE_RUNTIME_LIFECYCLE",
            v3::Capability::CorePaneLifecycle => "CORE_PANE_LIFECYCLE",
            v3::Capability::CoreTerminalIo => "CORE_TERMINAL_IO",
            v3::Capability::CoreTerminalModes => "CORE_TERMINAL_MODES",
            v3::Capability::CorePasteIntent => "CORE_PASTE_INTENT",
            v3::Capability::CoreFocusEvents => "CORE_FOCUS_EVENTS",
            other => {
                // Optional capabilities should not appear here, but handle gracefully
                match *other as i32 {
                    100 => "OPT_RUNTIME_INVENTORY_V2",
                    101 => "OPT_RUNTIME_TAKEOVER",
                    102 => "OPT_RESYNC",
                    103 => "OPT_CHUNKED_SCROLLBACK",
                    104 => "OPT_DIAGNOSTICS",
                    _ => "UNKNOWN",
                }
            }
        })
        .collect();
    v3::ProtocolError {
        kind: v3::ErrorKind::UnsupportedCapability as i32,
        message: format!("server missing required capabilities: {}", names.join(", ")),
        operation: "Handshake".into(),
        retryable: false,
        user_action_required: true,
        retry_after_seconds: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiate_version_exact_match() {
        assert_eq!(negotiate_version(3, 3, 3, 3).unwrap(), 3);
    }

    #[test]
    fn negotiate_version_picks_highest_overlap() {
        assert_eq!(negotiate_version(3, 5, 3, 4).unwrap(), 4);
        assert_eq!(negotiate_version(3, 4, 3, 5).unwrap(), 4);
    }

    #[test]
    fn negotiate_version_no_overlap_returns_error() {
        let err = negotiate_version(4, 5, 3, 3).unwrap_err();
        assert_eq!(err.kind, v3::ErrorKind::ProtocolMismatch as i32);
        assert!(err.user_action_required);
        assert!(err.message.contains("v4"));
        assert!(err.message.contains("v3"));
    }

    #[test]
    fn negotiate_version_client_below_server_returns_error() {
        let err = negotiate_version(3, 3, 5, 5).unwrap_err();
        assert_eq!(err.kind, v3::ErrorKind::ProtocolMismatch as i32);
    }

    #[test]
    fn validate_all_core_present() {
        let caps: Vec<i32> = CORE_CAPABILITIES.iter().map(|c| *c as i32).collect();
        assert!(validate_server_capabilities(&caps).is_ok());
    }

    #[test]
    fn validate_core_plus_optional_present() {
        let mut caps: Vec<i32> = CORE_CAPABILITIES.iter().map(|c| *c as i32).collect();
        caps.push(v3::Capability::OptDiagnostics as i32);
        assert!(validate_server_capabilities(&caps).is_ok());
    }

    #[test]
    fn validate_missing_single_core() {
        let caps: Vec<i32> = CORE_CAPABILITIES
            .iter()
            .filter(|c| **c != v3::Capability::CoreFocusEvents)
            .map(|c| *c as i32)
            .collect();
        let missing = validate_server_capabilities(&caps).unwrap_err();
        assert_eq!(missing, vec![v3::Capability::CoreFocusEvents]);
    }

    #[test]
    fn validate_missing_multiple_core() {
        let caps = vec![
            v3::Capability::CoreWorkspaceLifecycle as i32,
            v3::Capability::CoreTerminalIo as i32,
        ];
        let missing = validate_server_capabilities(&caps).unwrap_err();
        assert_eq!(missing.len(), 4);
        assert!(missing.contains(&v3::Capability::CorePaneLifecycle));
        assert!(missing.contains(&v3::Capability::CoreTerminalModes));
        assert!(missing.contains(&v3::Capability::CorePasteIntent));
        assert!(missing.contains(&v3::Capability::CoreFocusEvents));
    }

    #[test]
    fn validate_empty_caps_returns_all_core_missing() {
        let missing = validate_server_capabilities(&[]).unwrap_err();
        assert_eq!(missing.len(), CORE_CAPABILITIES.len());
    }

    #[test]
    fn effective_capabilities_intersection() {
        let client = vec![
            v3::Capability::CoreWorkspaceLifecycle as i32,
            v3::Capability::CorePaneLifecycle as i32,
            v3::Capability::OptDiagnostics as i32,
        ];
        let server = vec![
            v3::Capability::CoreWorkspaceLifecycle as i32,
            v3::Capability::CorePaneLifecycle as i32,
        ];
        let effective = effective_capabilities(&client, &server);
        assert_eq!(effective.len(), 2);
        assert!(!effective.contains(&(v3::Capability::OptDiagnostics as i32)));
    }

    #[test]
    fn effective_capabilities_empty_when_disjoint() {
        let client = vec![v3::Capability::OptDiagnostics as i32];
        let server = vec![v3::Capability::OptResync as i32];
        assert!(effective_capabilities(&client, &server).is_empty());
    }

    #[test]
    fn build_client_hello_populates_all_fields() {
        let id = uuid::Uuid::new_v4();
        let hello = build_client_hello(id, "rttx", "0.4.0", CORE_CAPABILITIES);
        assert_eq!(hello.min_protocol_version, V3_PROTOCOL_VERSION);
        assert_eq!(hello.max_protocol_version, V3_PROTOCOL_VERSION);
        assert_eq!(hello.client_id, crate::uuid_to_bytes(id));
        assert_eq!(hello.client_name, "rttx");
        assert_eq!(hello.client_version, "0.4.0");
        assert_eq!(hello.capabilities.len(), CORE_CAPABILITIES.len());
    }

    #[test]
    fn build_server_hello_populates_all_fields() {
        let id = uuid::Uuid::new_v4();
        let hello = build_server_hello(id, "0.4.0", 3, CORE_CAPABILITIES);
        assert_eq!(hello.negotiated_protocol_version, 3);
        assert_eq!(hello.server_id, crate::uuid_to_bytes(id));
        assert_eq!(hello.server_version, "0.4.0");
        assert_eq!(hello.capabilities.len(), CORE_CAPABILITIES.len());
    }

    #[test]
    fn missing_capabilities_error_lists_names() {
        let missing = vec![v3::Capability::CoreFocusEvents, v3::Capability::CorePasteIntent];
        let err = missing_capabilities_error(&missing);
        assert_eq!(err.kind, v3::ErrorKind::UnsupportedCapability as i32);
        assert!(err.message.contains("CORE_FOCUS_EVENTS"));
        assert!(err.message.contains("CORE_PASTE_INTENT"));
        assert!(err.user_action_required);
        assert!(!err.retryable);
    }

    #[test]
    fn core_capabilities_count() {
        assert_eq!(CORE_CAPABILITIES.len(), 6);
    }

    #[test]
    fn core_capabilities_values_in_range() {
        for cap in CORE_CAPABILITIES {
            let val = *cap as i32;
            assert!((1..100).contains(&val), "core capability {val} should be in 1–99");
        }
    }
}

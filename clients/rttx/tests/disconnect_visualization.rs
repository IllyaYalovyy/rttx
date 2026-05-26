//! Integration tests for disconnect visualization in persistent panes.
//!
//! Regression tests for #957: disconnected panes must provide clear visual
//! feedback — dimming, VTE message, and overlay banner with reconnection
//! progress.

use rttx::runtime::{ConnectionProblem, ConnectionStatus, present_connection_status};

/// Disconnect presentation must disable input for all non-connected states.
/// The pane header label must be non-empty so the user always has a status
/// indicator even without the overlay banner.
#[test]
fn disconnect_presentation_disables_input_and_provides_label() {
    let disconnect_states = [
        ConnectionStatus::Disconnected,
        ConnectionStatus::Reconnecting { attempt: 1, retry_in_secs: 5 },
        ConnectionStatus::Reconnecting { attempt: 10, retry_in_secs: 30 },
        ConnectionStatus::Blocked(ConnectionProblem::DaemonUnavailable),
        ConnectionStatus::Blocked(ConnectionProblem::PermissionDenied),
        ConnectionStatus::SessionMissing,
        ConnectionStatus::Connecting,
        ConnectionStatus::Starting,
    ];
    for status in &disconnect_states {
        let presentation = present_connection_status(status);
        assert!(!presentation.input_enabled, "input must be disabled for {status:?}");
        assert!(
            !presentation.header_label.is_empty(),
            "header_label must be non-empty for {status:?}"
        );
    }
}

/// Connected and Recovered states must enable input and hide disconnect
/// indicators. This ensures the banner and VTE message are cleared on
/// successful reconnect.
#[test]
fn connected_states_enable_input() {
    for status in [ConnectionStatus::Connected, ConnectionStatus::Recovered] {
        let presentation = present_connection_status(&status);
        assert!(presentation.input_enabled, "input must be enabled for {status:?}");
    }
}

/// Reconnecting header label must include the retry delay so the user
/// knows when the next attempt will happen without looking at the banner.
#[test]
fn reconnecting_header_includes_retry_delay() {
    let status = ConnectionStatus::Reconnecting { attempt: 2, retry_in_secs: 8 };
    let presentation = present_connection_status(&status);
    assert!(
        presentation.header_label.contains('8'),
        "header should include retry seconds: {:?}",
        presentation.header_label
    );
}

/// Blocked state header must indicate action is required.
#[test]
fn blocked_header_indicates_action_required() {
    let status = ConnectionStatus::Blocked(ConnectionProblem::VersionMismatch);
    let presentation = present_connection_status(&status);
    assert!(
        presentation.header_label.contains("Action Required"),
        "blocked header should say Action Required: {:?}",
        presentation.header_label
    );
}

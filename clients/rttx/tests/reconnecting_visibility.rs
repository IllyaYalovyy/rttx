//! Integration tests for reconnecting workspace tab visibility.
//!
//! Regression tests for #935: workspace tabs must be visually distinct
//! during reconnect (warning/yellow) vs initial connect (dim-label/gray),
//! and the circuit breaker must fire within a reasonable time.

use rttx::runtime::{
    ConnectionProblem, ConnectionStatus, RuntimeEndpoint, connection_icon,
    present_connection_status,
};

/// Reconnecting state must use a different CSS class than Connecting
/// so the user can distinguish a failing retry loop from a healthy
/// initial connection.
#[test]
fn reconnecting_visually_distinct_from_connecting() {
    let ep = RuntimeEndpoint::Local;
    let connecting = connection_icon(&ep, &ConnectionStatus::Connecting, true);
    let reconnecting = connection_icon(
        &ep,
        &ConnectionStatus::Reconnecting { attempt: 2, retry_in_secs: 4 },
        true,
    );

    assert_ne!(
        connecting.css_class, reconnecting.css_class,
        "Reconnecting must use a different CSS class than Connecting"
    );
    assert_eq!(connecting.css_class, "dim-label");
    assert_eq!(reconnecting.css_class, "warning");
}

/// Reconnecting tooltip must differ from Connecting tooltip to provide
/// diagnostic information about the retry state.
#[test]
fn reconnecting_tooltip_differs_from_connecting() {
    let ep = RuntimeEndpoint::remote("host");
    let connecting = connection_icon(&ep, &ConnectionStatus::Connecting, true);
    let reconnecting = connection_icon(
        &ep,
        &ConnectionStatus::Reconnecting { attempt: 1, retry_in_secs: 3 },
        true,
    );

    assert_ne!(
        connecting.tooltip, reconnecting.tooltip,
        "Reconnecting tooltip must differ from Connecting tooltip"
    );
}

/// Blocked state must clearly indicate that manual action is needed.
#[test]
fn blocked_state_guides_user_to_retry() {
    let ep = RuntimeEndpoint::Local;
    let icon = connection_icon(
        &ep,
        &ConnectionStatus::Blocked(ConnectionProblem::DaemonUnavailable),
        true,
    );

    assert_eq!(icon.css_class, "error");
    assert!(
        icon.tooltip.contains("retry"),
        "Blocked tooltip must guide user to retry: {:?}",
        icon.tooltip
    );
}

/// The visual progression from Connecting → Reconnecting → Blocked must
/// use increasingly alarming colors: gray → yellow → red.
#[test]
fn connection_status_color_progression() {
    let ep = RuntimeEndpoint::Local;

    let connecting = connection_icon(&ep, &ConnectionStatus::Connecting, true);
    let reconnecting = connection_icon(
        &ep,
        &ConnectionStatus::Reconnecting { attempt: 5, retry_in_secs: 5 },
        true,
    );
    let blocked = connection_icon(
        &ep,
        &ConnectionStatus::Blocked(ConnectionProblem::DaemonUnavailable),
        true,
    );

    assert_eq!(connecting.css_class, "dim-label", "initial connect = gray");
    assert_eq!(reconnecting.css_class, "warning", "reconnecting = yellow/warning");
    assert_eq!(blocked.css_class, "error", "blocked = red/error");
}

/// Reconnecting pane header must show retry countdown information.
#[test]
fn reconnecting_header_shows_retry_info() {
    let status = ConnectionStatus::Reconnecting { attempt: 3, retry_in_secs: 7 };
    let presentation = present_connection_status(&status);

    assert!(
        presentation.header_label.contains('7'),
        "header should show remaining seconds: {:?}",
        presentation.header_label
    );
    assert!(!presentation.input_enabled);
}

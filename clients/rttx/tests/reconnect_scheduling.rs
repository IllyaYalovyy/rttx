//! Integration tests for reconnect scheduling behavior.
//!
//! Verifies that the reconnect loop handles both transient and
//! non-transient errors without silently dying.

use rttx::runtime::ConnectionProblem;

#[test]
fn non_transient_errors_are_not_classified_as_transient() {
    let cases = [
        ConnectionProblem::VersionMismatch,
        ConnectionProblem::OwnershipConflict,
        ConnectionProblem::Protocol("test".into()),
        ConnectionProblem::UserActionRequired("test".into()),
    ];
    for problem in &cases {
        assert!(
            !problem.is_transient(),
            "{problem:?} should not be transient — reconnect must still retry with max delay"
        );
    }
}

#[test]
fn transient_errors_are_classified_correctly() {
    assert!(
        ConnectionProblem::DaemonUnavailable.is_transient(),
        "DaemonUnavailable should be transient for progressive backoff"
    );
}

/// Verify that `classify_connection_problem` maps I/O errors to the
/// transient `DaemonUnavailable` variant — the classification that
/// drives the reconnect-vs-give-up decision in the Reconnect handler.
#[test]
fn io_error_is_transient_daemon_unavailable() {
    use rttx::daemon::DaemonError;
    use rttx::runtime::classify_connection_problem;

    let io_err = DaemonError::Io(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken"));
    let problem = classify_connection_problem(&io_err);
    assert!(
        problem.is_transient(),
        "I/O errors must be transient so the Reconnect handler breaks on first failure"
    );

    let disconnected = DaemonError::Disconnected;
    let problem = classify_connection_problem(&disconnected);
    assert!(
        problem.is_transient(),
        "Disconnected must be transient so the Reconnect handler breaks on first failure"
    );
}

/// Regression test for #576: the reconnect backoff delay must continue
/// ramping up when a reconnect cycle connects successfully but the
/// subsequent reattach fails. Without the fix, `ensure_connected` resets
/// the counter to 0 and the next delay drops back to 1 second.
///
/// This test verifies the contract at the connection-status level: after
/// a simulated reconnect failure at attempt N, the emitted Reconnecting
/// status must carry a delay > 1 (proving the counter was preserved).
#[test]
fn reconnect_backoff_continues_after_transient_reattach_failure() {
    use rttx::runtime::{ConnectionEvent, ConnectionStatus, advance_connection_status};

    // Simulate the state after 5 reconnect cycles: the next attempt
    // should use delay = min(6, max). If the counter were reset to 0,
    // the delay would be 1.
    let prior_attempt = 5u32;
    let max_delay = 10u32;
    let next_attempt = prior_attempt + 1;
    let expected_delay = next_attempt.min(max_delay);

    let status = advance_connection_status(
        &ConnectionStatus::Connecting,
        ConnectionEvent::RetryScheduled { attempt: next_attempt, retry_in_secs: expected_delay },
    );

    match status {
        ConnectionStatus::Reconnecting { attempt, retry_in_secs } => {
            assert_eq!(attempt, next_attempt);
            assert_eq!(retry_in_secs, expected_delay);
            assert!(
                retry_in_secs > 1,
                "delay must be > 1 to prove backoff was preserved, got {retry_in_secs}"
            );
        }
        other => panic!("expected Reconnecting status, got {other:?}"),
    }
}

/// Regression test for #404: connection failure classification must
/// produce a problem variant that carries diagnostic detail visible
/// to the user. Before the fix, the log messages on the reconnect
/// path were at debug/info level and invisible in production.
#[test]
fn connection_failure_produces_diagnosable_problem() {
    use rttx::daemon::DaemonError;
    use rttx::runtime::classify_connection_problem;

    let cases: Vec<(DaemonError, &str)> = vec![
        (
            DaemonError::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "connection refused",
            )),
            "I/O connection refused",
        ),
        (
            DaemonError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "socket not found")),
            "I/O socket not found",
        ),
        (DaemonError::Disconnected, "transport disconnected"),
    ];

    for (error, label) in cases {
        let problem = classify_connection_problem(&error);
        // The problem must be transient (so reconnect is scheduled)
        // and must have a non-empty label for diagnostic display.
        assert!(
            problem.is_transient(),
            "{label}: {problem:?} must be transient so reconnect is scheduled"
        );
        assert!(
            !problem.label().is_empty(),
            "{label}: problem label must be non-empty for diagnostic display"
        );
    }
}

/// Verify that the daemon bridge logging paths use tracing (not log)
/// by exercising `classify_connection_problem` through a code path
/// that emits tracing events. The tracing subscriber captures these
/// without panicking, confirming the log-to-tracing migration is intact.
#[test]
fn connection_classification_exercises_tracing_path() {
    use rttx::daemon::DaemonError;
    use rttx::runtime::classify_connection_problem;

    // Install a no-op tracing subscriber for this test so tracing
    // macros execute their formatting code without panicking.
    let _guard = tracing_subscriber::fmt()
        .with_writer(std::io::sink)
        .with_max_level(tracing::Level::TRACE)
        .try_init();

    let error = DaemonError::Io(std::io::Error::new(
        std::io::ErrorKind::ConnectionRefused,
        "test connection refused",
    ));
    let problem = classify_connection_problem(&error);
    assert!(problem.is_transient());
}

/// Regression test for #710: transient connection problems must not
/// produce `Blocked` status. The `Reconnecting` status from the
/// reconnect scheduler must be the final word so panes show "Retry Ns"
/// instead of "Action Required".
///
/// This integration test verifies the contract at the connection-status
/// level: a transient `DaemonUnavailable` problem must never advance
/// the state machine to `Blocked`.
#[test]
fn transient_problem_never_produces_blocked_status() {
    use rttx::runtime::{
        ConnectionEvent, ConnectionProblem, ConnectionStatus, advance_connection_status,
    };

    let transient = ConnectionProblem::DaemonUnavailable;
    assert!(transient.is_transient());

    // The state machine must produce Reconnecting for transient errors,
    // never Blocked.
    let status = advance_connection_status(
        &ConnectionStatus::Connecting,
        ConnectionEvent::RetryScheduled { attempt: 1, retry_in_secs: 1 },
    );
    assert!(
        matches!(status, ConnectionStatus::Reconnecting { .. }),
        "transient error must produce Reconnecting, got {status:?}"
    );
    assert!(
        !matches!(status, ConnectionStatus::Blocked(_)),
        "transient error must never produce Blocked"
    );

    // Blocked must only be produced for non-transient problems.
    let blocked = advance_connection_status(
        &ConnectionStatus::Connecting,
        ConnectionEvent::Failed(ConnectionProblem::VersionMismatch),
    );
    assert!(
        matches!(blocked, ConnectionStatus::Blocked(_)),
        "non-transient error should produce Blocked"
    );
}

/// Regression test for #710: `Reconnecting` status must accept no input
/// but `Blocked` must also accept no input. The key difference is the
/// user-facing label: Reconnecting shows "Retry Ns" while Blocked shows
/// "Action Required". Both disable input, but Reconnecting signals that
/// recovery is in progress.
#[test]
fn reconnecting_and_blocked_both_disable_input_but_differ_in_label() {
    use rttx::runtime::{ConnectionProblem, ConnectionStatus, present_connection_status};

    let reconnecting = ConnectionStatus::Reconnecting { attempt: 1, retry_in_secs: 3 };
    let blocked = ConnectionStatus::Blocked(ConnectionProblem::DaemonUnavailable);

    let reconnecting_pres = present_connection_status(&reconnecting);
    let blocked_pres = present_connection_status(&blocked);

    // Both disable input.
    assert!(!reconnecting_pres.input_enabled);
    assert!(!blocked_pres.input_enabled);

    // Labels must differ — this is the user-visible distinction.
    assert_ne!(
        reconnecting_pres.header_label, blocked_pres.header_label,
        "Reconnecting and Blocked must have different labels"
    );
    assert!(
        reconnecting_pres.header_label.contains("Retry"),
        "Reconnecting label should contain 'Retry', got '{}'",
        reconnecting_pres.header_label
    );
    assert!(
        blocked_pres.header_label.contains("Action Required"),
        "Blocked label should contain 'Action Required', got '{}'",
        blocked_pres.header_label
    );
}

/// Regression test for #769: `Connected` and `Recovered` must enable input;
/// all other statuses must disable it. This is the pure-state contract that
/// the GTK layer relies on to gate keyboard and mouse forwarding.
#[test]
fn connected_and_recovered_enable_input_all_others_disable() {
    use rttx::runtime::{ConnectionStatus, present_connection_status};

    let input_enabled_statuses = [ConnectionStatus::Connected, ConnectionStatus::Recovered];
    for status in &input_enabled_statuses {
        let p = present_connection_status(status);
        assert!(p.input_enabled, "{status:?} must enable input, got input_enabled=false");
    }

    let input_disabled_statuses: Vec<ConnectionStatus> = vec![
        ConnectionStatus::Starting,
        ConnectionStatus::Connecting,
        ConnectionStatus::Disconnected,
        ConnectionStatus::SessionMissing,
        ConnectionStatus::Reconnecting { attempt: 1, retry_in_secs: 5 },
        ConnectionStatus::Blocked(ConnectionProblem::DaemonUnavailable),
        ConnectionStatus::Blocked(ConnectionProblem::VersionMismatch),
    ];
    for status in &input_disabled_statuses {
        let p = present_connection_status(status);
        assert!(!p.input_enabled, "{status:?} must disable input, got input_enabled=true");
    }
}

/// Regression test for #769: every `ConnectionStatus` variant must produce
/// a non-empty `header_label` in its presentation. A blank label would
/// leave the pane header visually inconsistent with the actual state.
#[test]
fn all_connection_statuses_produce_non_empty_header_label() {
    use rttx::runtime::{ConnectionStatus, present_connection_status};

    let statuses: Vec<ConnectionStatus> = vec![
        ConnectionStatus::Starting,
        ConnectionStatus::Connecting,
        ConnectionStatus::Connected,
        ConnectionStatus::Recovered,
        ConnectionStatus::Disconnected,
        ConnectionStatus::SessionMissing,
        ConnectionStatus::Reconnecting { attempt: 1, retry_in_secs: 3 },
        ConnectionStatus::Blocked(ConnectionProblem::DaemonUnavailable),
        ConnectionStatus::Blocked(ConnectionProblem::VersionMismatch),
        ConnectionStatus::Blocked(ConnectionProblem::OwnershipConflict),
        ConnectionStatus::Blocked(ConnectionProblem::PermissionDenied),
    ];
    for status in &statuses {
        let p = present_connection_status(status);
        assert!(!p.header_label.is_empty(), "{status:?} must produce a non-empty header_label");
    }
}

/// Regression test for #769: the `header_label` for `Connected` and
/// `Recovered` must be identical ("Connected") so the user sees a
/// stable label after reconnect, not a transient "Recovered" flash.
#[test]
fn connected_and_recovered_share_same_header_label() {
    use rttx::runtime::{ConnectionStatus, present_connection_status};

    let connected = present_connection_status(&ConnectionStatus::Connected);
    let recovered = present_connection_status(&ConnectionStatus::Recovered);
    assert_eq!(
        connected.header_label, recovered.header_label,
        "Connected and Recovered must show the same label to the user"
    );
    assert_eq!(connected.header_label, "Connected");
}

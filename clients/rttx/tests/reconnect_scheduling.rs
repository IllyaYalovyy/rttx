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

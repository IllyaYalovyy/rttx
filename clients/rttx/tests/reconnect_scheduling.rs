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

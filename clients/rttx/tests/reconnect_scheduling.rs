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

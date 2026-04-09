//! Integration tests for flock-based single-instance enforcement.

use rttx_server::single_instance::{SingleInstanceError, SingleInstanceGuard};

#[test]
fn concurrent_acquisition_rejects_second() {
    let dir = tempfile::tempdir().unwrap();
    let lock_path = dir.path().join("rttx-server.lock");

    let first = SingleInstanceGuard::try_acquire(&lock_path).unwrap();
    let second = SingleInstanceGuard::try_acquire(&lock_path);

    assert!(
        matches!(&second, Err(SingleInstanceError::AlreadyRunning)),
        "second acquisition must fail with AlreadyRunning, got: {second:?}"
    );

    drop(first);

    // After the first guard is dropped, acquisition should succeed again.
    let third = SingleInstanceGuard::try_acquire(&lock_path);
    assert!(third.is_ok(), "third acquisition should succeed after first is dropped");
}

#[test]
fn stale_lock_file_does_not_block() {
    let dir = tempfile::tempdir().unwrap();
    let lock_path = dir.path().join("rttx-server.lock");

    // Create a lock file without holding a lock (simulates stale file after crash).
    std::fs::write(&lock_path, "").unwrap();

    let guard = SingleInstanceGuard::try_acquire(&lock_path);
    assert!(guard.is_ok(), "stale lock file should not prevent acquisition");
}

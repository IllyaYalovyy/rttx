//! Flock-based single-instance enforcement for `rttx-server`.
//!
//! Acquires an exclusive advisory lock on a lock file in the workspace directory.
//! The lock is held for the process lifetime and released automatically on drop
//! (or process exit/crash). A second instance attempting to acquire the lock
//! fails immediately with [`AlreadyRunning`].

use nix::fcntl::{Flock, FlockArg};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

/// Held for the lifetime of the daemon process. Dropping it releases the lock.
#[derive(Debug)]
pub struct SingleInstanceGuard {
    _lock: Flock<File>,
    path: PathBuf,
}

impl SingleInstanceGuard {
    /// Try to acquire the single-instance lock.
    ///
    /// Creates the lock file if it does not exist. Returns the guard on success,
    /// or [`AlreadyRunning`] if another instance holds the lock.
    pub fn try_acquire(lock_path: &Path) -> Result<Self, SingleInstanceError> {
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = File::options().create(true).truncate(false).write(true).open(lock_path)?;

        let lock = Flock::lock(file, FlockArg::LockExclusiveNonblock).map_err(|(_, errno)| {
            if errno == nix::errno::Errno::EWOULDBLOCK {
                SingleInstanceError::AlreadyRunning
            } else {
                SingleInstanceError::Io(io::Error::from_raw_os_error(errno as i32))
            }
        })?;

        Ok(Self { _lock: lock, path: lock_path.to_owned() })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SingleInstanceError {
    #[error("another instance is already running")]
    AlreadyRunning,
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_instance_acquires_lock() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("test.lock");
        let guard = SingleInstanceGuard::try_acquire(&lock_path);
        assert!(guard.is_ok(), "first instance should acquire the lock");
        assert!(lock_path.exists());
    }

    #[test]
    fn second_instance_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("test.lock");
        let _first = SingleInstanceGuard::try_acquire(&lock_path).unwrap();
        let second = SingleInstanceGuard::try_acquire(&lock_path);
        assert!(
            matches!(second, Err(SingleInstanceError::AlreadyRunning)),
            "second instance should get AlreadyRunning"
        );
    }

    #[test]
    fn lock_released_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("test.lock");
        {
            let _guard = SingleInstanceGuard::try_acquire(&lock_path).unwrap();
        }
        // After drop, a new instance should succeed.
        let guard = SingleInstanceGuard::try_acquire(&lock_path);
        assert!(guard.is_ok(), "lock should be available after first guard is dropped");
    }

    #[test]
    fn creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("nested").join("dir").join("test.lock");
        let guard = SingleInstanceGuard::try_acquire(&lock_path);
        assert!(guard.is_ok(), "should create parent dirs and acquire lock");
    }

    #[test]
    fn guard_reports_lock_path() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("test.lock");
        let guard = SingleInstanceGuard::try_acquire(&lock_path).unwrap();
        assert_eq!(guard.path(), lock_path);
    }
}

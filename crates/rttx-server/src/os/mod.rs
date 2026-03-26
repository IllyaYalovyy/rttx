//! OS abstraction layer for platform-specific operations.

pub mod unix;

use std::path::PathBuf;

/// Trait abstracting OS-level operations for testability.
pub trait OsInterface: Send + Sync {
    /// Return the runtime directory for the server socket.
    fn runtime_dir(&self) -> PathBuf;

    /// Return the cache directory for persistent state and scrollback logs.
    fn cache_dir(&self) -> PathBuf;
}

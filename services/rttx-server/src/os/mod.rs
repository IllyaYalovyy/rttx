//! OS abstraction layer for platform-specific operations.

pub mod unix;

use std::path::PathBuf;

/// Trait abstracting OS-level operations for testability.
pub trait OsInterface: Send + Sync {
    /// Return the workspace directory for the server socket.
    fn runtime_dir(&self) -> PathBuf;

    /// Return the cache directory for persistent state and scrollback logs.
    ///
    /// This is the v1 storage location (`$XDG_CACHE_HOME/rttx-server/`).
    /// New code should prefer [`state_dir`](Self::state_dir) for durable state
    /// that must survive cache cleanup.
    fn cache_dir(&self) -> PathBuf;

    /// Return the daemon state directory under `$XDG_STATE_HOME`.
    ///
    /// Layout: `$XDG_STATE_HOME/rttx/daemon/` (production) or
    /// `$XDG_STATE_HOME/rttx-devel/daemon/` (dev mode).
    ///
    /// RFC-022 owns everything under `daemon/`. RFC-023 owns `client/`.
    fn state_dir(&self) -> PathBuf;
}

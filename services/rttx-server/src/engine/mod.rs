//! Engine for pane process management.
//!
//! `NativeEngine` owns PTYs directly, spawning shell processes and managing
//! their lifecycle.

pub mod native;

use crate::pty::Pty;
use uuid::Uuid;

/// Configuration for spawning a pane.
#[derive(Debug, Clone)]
pub struct PaneSpawnConfig {
    /// Command to run (default: user's shell).
    pub command: Vec<String>,
    /// Working directory.
    pub cwd: Option<String>,
    /// Extra environment variables.
    pub env: Vec<(String, String)>,
    /// Terminal columns.
    pub cols: u16,
    /// Terminal rows.
    pub rows: u16,
}

/// Trait for pane process engines.
pub trait Engine: Send + Sync {
    /// Spawn a new pane process, returning the PTY handle.
    fn spawn_pane(
        &self,
        pane_id: Uuid,
        config: &PaneSpawnConfig,
    ) -> Result<Pty, crate::pty::PtyError>;
}
